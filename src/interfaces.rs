use rustbus::params;
use rustbus::{Base, Container, Param};
use std::collections::HashMap;

pub const PROP_IF_STR: &'static str = "org.freedesktop.DBus.Properties";
pub const OBJ_MANAGER_IF_STR: &'static str = "org.freedesktop.DBus.GetManagedObjects";
pub const SERV_IF_STR: &'static str = "org.bluez.GattService1";
pub const CHAR_IF_STR: &'static str = "org.bluez.GattCharacteristic1";
pub const DESC_IF_STR: &'static str = "org.bluez.GattDescriptor1";
pub const DEV_IF_STR: &'static str = "org.bluez.Device1";
pub const MANAGER_IF_STR: &'static str = "org.bluez.GattManager1";
pub const LEAD_IF_STR: &'static str = "org.bluez.LEAdvertisement1";
pub const INTRO_IF_STR: &'static str = "org.freedesktop.DBus.Introspectable";

pub const UUID_PROP: &'static str = "UUID";
pub const SERVICE_PROP: &'static str = "Service";
pub const VALUE_PROP: &'static str = "Value";
pub const WRITE_ACQUIRED_PROP: &'static str = "WriteAcquired";
pub const NOTIFY_ACQUIRED_PROP: &'static str = "NotifyAcquired";
pub const NOTIFYING_PROP: &'static str = "Notifying";
pub const FLAGS_PROP: &'static str = "Flags";
pub const HANDLE_PROP: &'static str = "Handle";
pub const CHAR_PROP: &'static str = "Characteristic";
pub const PRIMARY_PROP: &'static str = "Primary";
pub const DEVICE_PROP: &'static str = "Device";
pub const INCLUDES_PROP: &'static str = "Includes";
pub const TYPE_PROP: &'static str = "Type";
pub const SERV_UUIDS_PROP: &'static str = "ServiceUUIDs";
pub const SOLICIT_UUIDS_PROP: &'static str = "SolicitUUIDs";
pub const SERV_DATA_PROP: &'static str = "ServiceData";
pub const DATA_PROP: &'static str = "Data";
pub const MANU_DATA_PROP: &'static str = "ManufacturererData";
pub const DISCOVERABLE_PROP: &'static str = "Discoverable";
pub const DISCOVERABLE_TO_PROP: &'static str = "DiscoverableTimeout";
pub const LOCAL_NAME_PROP: &'static str = "LocalName";
pub const APPEARANCE_PROP: &'static str = "Appearance";
pub const DURATION_PROP: &'static str = "Duration";
pub const TO_PROP: &'static str = "Timeout";
pub const SND_CHANNEL_PROP: &'static str = "SecondaryChannel";

pub(crate) const SERV_IF_PROPS: &[&'static str] =
    &[UUID_PROP, PRIMARY_PROP, DEVICE_PROP, HANDLE_PROP]; // HANDLE_PROP is not used
pub(crate) const CHAR_IF_PROPS: &[&'static str] = &[
    UUID_PROP,
    SERVICE_PROP,
    VALUE_PROP,
    WRITE_ACQUIRED_PROP,
    NOTIFY_ACQUIRED_PROP,
    NOTIFYING_PROP,
    FLAGS_PROP,
    HANDLE_PROP,
];
pub(crate) const DESC_IF_PROPS: &[&'static str] =
    &[UUID_PROP, VALUE_PROP, FLAGS_PROP, HANDLE_PROP, CHAR_PROP];

pub(crate) const LEAD_IF_PROPS: &[&'static str] = &[
    TYPE_PROP,
    SERV_UUIDS_PROP,
    MANU_DATA_PROP,
    SERV_DATA_PROP,
    // DATA_PROP,
    /* TODO: implement: DISCOVERABLE_PROP,
    DISCOVERABLE_TO_PROP,*/
    INCLUDES_PROP,
    LOCAL_NAME_PROP,
    APPEARANCE_PROP,
    DURATION_PROP,
    TO_PROP,
    //SND_CHANNEL_PROP,
];

pub(crate) const PROP_IF: (&'static str, &[&'static str]) = (PROP_IF_STR, &[]);
pub(crate) const SERV_IF: (&'static str, &[&'static str]) = (SERV_IF_STR, SERV_IF_PROPS);
pub(crate) const CHAR_IF: (&'static str, &[&'static str]) = (CHAR_IF_STR, CHAR_IF_PROPS);
pub(crate) const DESC_IF: (&'static str, &[&'static str]) = (DESC_IF_STR, DESC_IF_PROPS);
pub(crate) const LEAD_IF: (&'static str, &[&'static str]) = (LEAD_IF_STR, LEAD_IF_PROPS);

pub const BLUEZ_DEST: &'static str = "org.bluez";

pub const PROP_CHANGED_SIG: &'static str = "PropertiesChanged";
pub const MANGAGED_OBJ_CALL: &'static str = "GetManagedObjects";
pub const REGISTER_CALL: &'static str = "RegisterApplication";
pub const GET_ALL_CALL: &'static str = "GetAll";

// Bluez Errors
pub const BLUEZ_NOT_PERM: &'static str = "org.bluez.Error.NotPermitted";
pub const BLUEZ_FAILED: &'static str = "org.bluez.Error.Failed";

// Standard DBus Errors
pub const UNKNOWN_METHOD: &'static str = "org.dbus.freedesktop.UnknownMethod";

pub const IF_ADDED_SIG: &'static str = "InterfaceAdded";
pub const IF_REMOVED_SIG: &'static str = "InterfaceRemoved";

pub(crate) fn if_dict_to_map<'a, 'b>(
    if_dict: Param<'a, 'b>,
) -> HashMap<String, HashMap<String, params::Variant<'a, 'b>>> {
    if let Param::Container(Container::Dict(if_dict)) = if_dict {
        let if_map: HashMap<String, HashMap<String, params::Variant>> = if_dict
            .map
            .into_iter()
            .filter_map(|(k, v)| {
                if let Base::String(key_str) = k {
                    if let Param::Container(Container::Dict(prop_dict)) = v {
                        let prop_map: HashMap<String, params::Variant> = prop_dict
                            .map
                            .into_iter()
                            .filter_map(|(k, v)| {
                                if let Base::String(key_str) = k {
                                    if let Param::Container(Container::Variant(var)) = v {
                                        return Some((key_str, *var));
                                    }
                                }
                                None
                            })
                            .collect();
                        return Some((key_str, prop_map));
                    }
                }
                None
            })
            .collect();
        if_map
    } else {
        panic!("Bad input to if_dict_to_map()");
    }
}
