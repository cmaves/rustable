use async_rustbus::rustbus_core;
use rustbus_core::message_builder::MarshalledMessage;
use rustbus_core::signature;
use rustbus_core::standard_messages;
use std::collections::HashMap;

use crate::BluezOptions;

use crate::path::ObjectPath;

#[derive(Debug)]
pub enum PropError {
    InterfaceNotFound,
    PropertyNotFound,
    InvalidValue,
    PermissionDenied,
}
impl PropError {
    fn to_str(&self) -> &'static str {
        match self {
            PropError::InvalidValue => "InvalidValue",
            PropError::PropertyNotFound => "PropertyNotFound",
            PropError::PermissionDenied => "PermissionDenied",
            PropError::InterfaceNotFound => "InterfaceNotFound",
        }
    }
}
pub trait Properties {
    const GET_ALL_ITEM: signature::Type = signature::Type::Container(signature::Container::Variant);
    fn get_all_type() -> signature::Type {
        signature::Type::Container(signature::Container::Dict(
            signature::Base::String,
            Box::new(Self::GET_ALL_ITEM),
        ))
    }
    const INTERFACES: &'static [(&'static str, &'static [&'static str])];
    fn properties_call(&mut self, msg: &MarshalledMessage) -> MarshalledMessage {
        match msg.dynheader.member.as_ref().unwrap().as_ref() {
            "Get" => self.get(msg),
            "Set" => self.set(msg),
            "GetAll" => self.get_all(msg),
            _ => standard_messages::unknown_method(&msg.dynheader),
        }
    }
    fn get_all_inner(
        &mut self,
        path: &ObjectPath,
        interface: &str,
    ) -> Result<HashMap<&'static str, BluezOptions<'static, 'static>>, PropError> {
        let props = Self::INTERFACES
            .iter()
            .find(|i| interface == i.0)
            .ok_or(PropError::InterfaceNotFound)
            .map(|i| i.1)?;
        let mut prop_map = HashMap::new();
        for prop in props {
            //eprintln!("{}: {}", interface, prop);
            let val = self.get_inner(path, interface, prop).unwrap();
            prop_map.insert(*prop, val);
        }
        Ok(prop_map)
    }
    fn get_all(&mut self, msg: &MarshalledMessage) -> MarshalledMessage {
        let interface = match msg.body.parser().get() {
            Ok(i) => i,
            Err(_) => {
                return msg
                    .dynheader
                    .make_error_response("InvalidArgs".to_string(), None)
            }
        };
        let path = ObjectPath::new(msg.dynheader.object.as_ref().unwrap()).unwrap();
        match self.get_all_inner(path, interface) {
            Ok(map) => {
                let mut res = msg.dynheader.make_response();
                res.body.push_param(map).unwrap();
                res
            }
            Err(e) => {
                return msg
                    .dynheader
                    .make_error_response(e.to_str().to_string(), None)
            }
        }
    }
    fn get(&mut self, msg: &MarshalledMessage) -> MarshalledMessage {
        let (interface, prop) = match msg.body.parser().get2() {
            Ok(out) => out,
            Err(_) => {
                return msg
                    .dynheader
                    .make_error_response("InvalidArgs".to_string(), None)
            }
        };
        let path = ObjectPath::new(msg.dynheader.object.as_ref().unwrap()).unwrap();
        match self.get_inner(path, interface, prop) {
            Ok(var) => {
                let mut reply = msg.dynheader.make_response();
                reply.body.push_param(var).unwrap();
                reply
            }
            Err(e) => msg
                .dynheader
                .make_error_response(e.to_str().to_string(), None),
        }
    }
    /// Should returng a variant containing if the property is found. If it is not found then it returns None.
    fn get_inner(
        &mut self,
        path: &ObjectPath,
        interface: &str,
        prop: &str,
    ) -> Result<BluezOptions<'static, 'static>, PropError>;
    fn set_inner(
        &mut self,
        path: &ObjectPath,
        interface: &str,
        prop: &str,
        val: BluezOptions,
    ) -> Result<(), PropError>;
    fn set(&mut self, msg: &MarshalledMessage) -> MarshalledMessage {
        let (interface, prop, var): (&str, &str, BluezOptions) = match msg.body.parser().get3() {
            Ok(vals) => vals,
            Err(err) => {
                return msg.dynheader.make_error_response(
                    "InvalidParameters".to_string(),
                    Some(format!("{:?}", err)),
                )
            }
        };
        let path = ObjectPath::new(msg.dynheader.object.as_ref().unwrap()).unwrap();
        match self.set_inner(path, interface, prop, var) {
            Ok(_) => msg.dynheader.make_response(),
            Err(e) => msg
                .dynheader
                .make_error_response(e.to_str().to_string(), None),
        }
    }
    fn get_all_interfaces(
        &mut self,
        path: &ObjectPath,
    ) -> HashMap<&'static str, HashMap<&'static str, BluezOptions<'static, 'static>>> {
        let mut ret = HashMap::new();
        for (interface, _) in Self::INTERFACES {
            let prop_map = self.get_all_inner(path, interface).unwrap();
            ret.insert(*interface, prop_map);
        }
        ret
    }
}
/*
trait ObjectManager {
    fn get_managed_object(&self)
}*/
