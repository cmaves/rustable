use crate::{Bluetooth, Charactersitic, Descriptor, Service};
use rustbus::{Base, Message, Param};
use std::fmt::Write;
use std::path::Path;
const INTROSPECT_FMT_P1: &'static str = "<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\" \"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\">
 <node name=\"";
const INTROSPECT_FMT_P2: &'static str = "\">
\t\t<interface name=\"org.freedesktop.DBus.Introspectable\"/>
\t\t\t<method name=\"Introspect\">
\t\t\t\t<arg name=\"xml_data\" type=\"s\" direction=\"out\"/>
\t\t\t</method>
\t\t</interface>";

const INTROSPECT_FMT_P3: &'static str = " </node>";

const PROP_STR: &'static str = "\t\t<interface name=\"org.freedesktop.DBus.Properties\">
\t\t\t<method name=\"Get\">
\t\t\t\t<arg name=\"interface_name\" type=\"s\" direction=\"in\"/>
\t\t\t\t<arg name=\"property_name\" type=\"s\" direction=\"in\"/>
\t\t\t\t<arg name=\"value\" type=\"v\" direction=\"out\"/>
\t\t\t</method>
\t\t\t<method name=\"GetAll\">
\t\t\t\t<arg name=\"interface_name\" type=\"s\" direction=\"in\"/>
\t\t\t\t<arg name=\"props\" type=\"a{sv}\" direction=\"out\"/>
\t\t\t</method>
\t\t\t<method name=\"Set\">
\t\t\t\t<arg name=\"interface_name\" type=\"s\" direction=\"in\"/>
\t\t\t\t<arg name=\"property_name\" type=\"s\" direction=\"in\"/>
\t\t\t\t<arg name=\"value\" type=\"v\" direction=\"in\"/>
\t\t\t</method>
\t\t</interface>";
const SERVICE_STR: &'static str = "\t\t<interface name=\"org.bluez.GattService1\">
\t\t\t<
\t\t\t<property name=\"UUID\" type=\"s\" access=\"read\"/>
\t\t\t<property name=\"Primary\" type=\"b\" access=\"read\"/>
\t\t\t<property name=\"Device\" type=\"o\" access=\"read\"/>
\t\t\t<property name=\"Handle\" type=\"q\" access=\"read\"/>
\t\t</interface>";
const CHAR_STR: &'static str = "\t\t<interface name=\"org.bluez.GattCharacteristic1\"\n>
\t\t\t<method name=\"ReadValue\">
\t\t\t\t<arg name=\"options\" type=\"a{sv}\", direction=\"in\"/>
\t\t\t\t<arg name=\"value\" type=\"ay\", direction=\"out\"/>
\t\t\t</method>
\t\t\t<method name=\"WriteValue\">
\t\t\t\t<arg name=\"value\" type=\"ay\", direction=\"in\"/>
\t\t\t\t<arg name=\"options\" type=\"a{sv}\", direction=\"in\"/>
\t\t\t</method>
\t\t\t<method name=\"AcquireWrite\">
\t\t\t\t<arg name=\"options\" type=\"a{sv}\", direction=\"in\"/>
\t\t\t\t<arg name=\"write_fd\" type=\"h\", direction=\"out\"/>
\t\t\t\t<arg name=\"mtu\" type=\"q\", direction=\"out\"/>
\t\t\t</method>
\t\t\t<method name=\"AcquireNotify\">
\t\t\t\t<arg name=\"options\" type=\"a{sv}\", direction=\"in\"/>
\t\t\t\t<arg name=\"write_fd\" type=\"h\", direction=\"out\"/>
\t\t\t\t<arg name=\"mtu\" type=\"q\", direction=\"out\"/>
\t\t\t</method>
\t\t\t<method name=\"StartNotify\"/>
\t\t\t<method name=\"StopNotify\"/>
\t\t\t<method name=\"Confirm\"/>
\t\t\t<property name=\"UUID\" type=\"s\" access=\"read\"/>
\t\t\t<property name=\"Service\" type=\"s\" access=\"read\"/>
\t\t\t<property name=\"Value\" type=\"ay\" access=\"read\"/>
\t\t\t<property name=\"WriteAcquired\" type=\"b\" access=\"read\"/>
\t\t\t<property name=\"NotifyAcquired\" type=\"b\" access=\"read\"/>
\t\t\t<property name=\"Notifying\" type=\"b\" access=\"read\"/>
\t\t\t<property name=\"Flags\" type=\"as\" access=\"read\"/>
\t\t\t<property name=\"Handle\" type=\"q\" access=\"readwrite\"/>
\t\t</interface>";
fn child_nodes(children: &[&str], dst: &mut String) {
    for child in children {
        write!(dst, "\t<node name=\"{}\"/>\n", child).unwrap();
    }
}
pub trait Introspectable {
    fn introspectable<'a, 'b>(&self, call: &Message<'a, 'b>) -> Message<'a, 'b> {
		let obj_path: &Path = call.object.as_ref().unwrap().as_ref();
        let mut reply = call.make_response();
        reply.push_param(Param::Base(Base::String(self.introspectable_str())));
        reply
    }
    fn introspectable_str(&self) -> String;
}

impl Introspectable for Bluetooth<'_, '_> {
    fn introspectable<'a, 'b>(&self, call: &Message<'a, 'b>) -> Message<'a, 'b> {
		let object: &Path = call.object.as_ref().unwrap().as_ref();
		let path = self.get_path();
		let stripped= object.strip_prefix(path.as_path()).unwrap();
        let mut reply = call.make_response();
		if let Some(child) = stripped.components().nth(0) {
			let mut xml = String::new();
			xml.push_str(&INTROSPECT_FMT_P1);
			xml.push_str(object.as_os_str().to_str().unwrap());
			xml.push_str(&INTROSPECT_FMT_P2);
			child_nodes(&[child.as_os_str().to_str().unwrap()], &mut xml);
			xml.push_str(&INTROSPECT_FMT_P3);
        	reply.push_param(Param::Base(Base::String(self.introspectable_str())));
        	reply
		} else {
        	reply.push_param(Param::Base(Base::String(self.introspectable_str())));
        	reply

		}
    }
    fn introspectable_str(&self) -> String {
        let mut ret = String::new();
        let path = self.get_path();
        ret.push_str(INTROSPECT_FMT_P1);
        ret.push_str(path.to_str().unwrap());
        ret.push_str(INTROSPECT_FMT_P2);
        ret.push_str(PROP_STR);
        let children: Vec<&str> = self
            .services
            .iter()
            .map(|s| &s.path[s.path.len() - 9..])
            .collect();
        child_nodes(&children, &mut ret);
        ret.push_str(INTROSPECT_FMT_P3);
        ret
    }
}
impl Introspectable for Charactersitic {
    fn introspectable_str(&self) -> String {
        let mut ret = String::new();
        ret.push_str(INTROSPECT_FMT_P1);
        ret.push_str(&self.path);
        ret.push_str(INTROSPECT_FMT_P2);
        ret.push_str(PROP_STR);
		ret.push_str(CHAR_STR);
        let children: Vec<&str> = self
            .descs
            .iter()
            .map(|s| &s.path[s.path.len() - 18..])
            .collect();
        child_nodes(&children, &mut ret);
        ret.push_str(INTROSPECT_FMT_P3);
        ret

    }
}
impl Introspectable for Service {
    fn introspectable_str(&self) -> String {
        unimplemented!()
    }
}
impl Introspectable for Descriptor {
    fn introspectable_str(&self) -> String {
        unimplemented!()
    }
}
