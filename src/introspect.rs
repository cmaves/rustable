use crate::Bluetooth;
use rustbus::message_builder::MarshalledMessage;
use rustbus::params::message::Message;
use std::fmt::Write;
use std::path::Path;
pub(crate) const INTROSPECT_FMT_P1: &'static str = "<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\" \"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\">
 <node name=\"";
pub(crate) const INTROSPECT_FMT_P2: &'static str = "\">
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
pub(crate) fn child_nodes(children: &[&str], dst: &mut String) {
    for child in children {
        write!(dst, "\t<node name=\"{}\"/>\n", child).unwrap();
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

impl Introspectable for Bluetooth {
    fn introspectable<'a, 'b>(&self, call: MarshalledMessage) -> MarshalledMessage {
        let object: &Path = call.dynheader.object.as_ref().unwrap().as_ref();
        let path = self.get_path();
        let stripped = path.strip_prefix(object).unwrap();
        let mut reply = call.dynheader.make_response();
        eprintln!("{:?}", stripped);
        if let Some(child) = stripped.components().nth(0) {
            // Handle if the introspection is for a parent of the Bluetooth dev
            eprintln!("{:?}", child);
            let mut xml = String::new();
            xml.push_str(&INTROSPECT_FMT_P1);
            xml.push_str(object.as_os_str().to_str().unwrap());
            xml.push_str(&INTROSPECT_FMT_P2);
            child_nodes(&[child.as_os_str().to_str().unwrap()], &mut xml);
            xml.push_str(&INTROSPECT_FMT_P3);
            reply.body.push_param(xml).unwrap();
            reply
        } else {
            // handle the normal case
            reply.body.push_param(self.introspectable_str()).unwrap();
            reply
        }
    }
    fn introspectable_str(&self) -> String {
        let mut ret = String::new();
        let path = self.get_path();
        ret.push_str(INTROSPECT_FMT_P1);
        ret.push_str(path.to_str().unwrap());
        ret.push_str(INTROSPECT_FMT_P2);
        ret.push_str(MANGAGER_STR);
        //ret.push_str(PROP_STR);
        let mut children: Vec<&str> = self
            .services
            .values()
            .map(|s| s.path.file_name().unwrap().to_str().unwrap())
            .collect();
        let ads = self
            .ads
            .iter()
            .map(|s| s.path.file_name().unwrap().to_str().unwrap());
        children.extend(ads);
        child_nodes(&children, &mut ret);
        ret.push_str(INTROSPECT_FMT_P3);
        ret
    }
}
