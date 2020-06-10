use crate::interfaces::*;
use crate::introspect::*;
use crate::*;

use rustbus::params;
use rustbus::signature;
use rustbus::wire::marshal_trait::Marshal;
use rustbus::Param;
use std::time::Instant;

pub struct Advertisement {
    pub typ: AdType,
    pub service_uuids: Vec<UUID>,
    pub manu_data: HashMap<String, ()>,
    pub solicit_uuids: Vec<UUID>,
    pub includes: Vec<String>,
    pub timeout: u16,
    pub duration: u16,
    pub(crate) index: u16,
    pub(crate) path: PathBuf,
    pub appearance: u16,
    pub localname: String,
    timeout_start: Instant,
    duration_start: Instant,
}
impl Advertisement {
    pub fn new(typ: AdType, localname: String) -> Self {
        let now = Instant::now();
        Advertisement {
            typ,
            service_uuids: Vec::new(),
            solicit_uuids: Vec::new(),
            includes: Vec::new(),
            timeout: 180,
            timeout_start: now,
            duration: 180,
            duration_start: now,
            appearance: 0,
            localname,
            index: 0,
            path: PathBuf::new(),
            manu_data: HashMap::new(),
        }
    }
    fn timeout(&self) -> u16 {
        let secs = Instant::now().duration_since(self.timeout_start).as_secs();
        (self.timeout as u64).saturating_sub(secs) as u16
    }
    fn duration(&self) -> u16 {
        let secs = Instant::now().duration_since(self.timeout_start).as_secs();
        (self.duration as u64).saturating_sub(secs) as u16
    }
    pub fn validate(&self) -> Result<(), Error> {
        for uuid in &self.service_uuids {
            if !validate_uuid(uuid) {
                return Err(Error::BadInput(format!(
                    "{} is an invalid uuid in service_uuids",
                    uuid
                )));
            }
        }
        for uuid in &self.solicit_uuids {
            if !validate_uuid(uuid) {
                return Err(Error::BadInput(format!(
                    "{} is an invalid uuid in solicit_uuids",
                    uuid
                )));
            }
        }

        Ok(())
    }
}
pub enum AdType {
    Peripheral,
    Broadcast,
}
impl ToString for AdType {
    fn to_string(&self) -> String {
        match self {
            AdType::Broadcast => "broadcast".to_string(),
            AdType::Peripheral => "peripheral".to_string(),
        }
    }
}
impl AdType {
    pub fn to_str(&self) -> &'static str {
        match self {
            AdType::Peripheral => "peripheral",
            AdType::Broadcast => "broadcast",
        }
    }
}

impl Properties for Advertisement {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[LEAD_IF, PROP_IF];
    fn get_inner<'a, 'b>(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>> {
        match interface {
            LEAD_IF_STR => match prop {
                TYPE_PROP => Some(Param::Base(self.typ.to_string().into())),
                SERV_UUIDS_PROP => {
                    let service_uuids: Vec<Param> = self
                        .service_uuids
                        .iter()
                        .map(|x| Param::Base(x.to_string().into()))
                        .collect();
                    let array = params::Array {
                        values: service_uuids,
                        element_sig: signature::Type::Base(signature::Base::String),
                    };
                    Some(Param::Container(Container::Array(array)))
                }
                MANU_DATA_PROP => {
                    //let manu_data = Param::Container(Container::Array(params::Array));
                    unimplemented!()
                }
                SOLICIT_UUIDS_PROP => unimplemented!(),
                SERV_DATA_PROP => unimplemented!(),
                /*TODO: implement: DATA_PROP => unimplemented!(),
                DISCOVERABLE_PROP => unimplemented!(),
                DISCOVERABLE_TO_PROP => unimplemented!(),*/
                INCLUDES_PROP => {
                    let includes: Vec<Param> = self
                        .includes
                        .iter()
                        .map(|x| Param::Base(x.to_string().into()))
                        .collect();
                    let array = params::Array {
                        values: includes,
                        element_sig: signature::Type::Base(signature::Base::String),
                    };
                    Some(Param::Container(Container::Array(array)))
                }
                LOCAL_NAME_PROP => Some(Param::Base(self.localname.to_string().into())),
                APPEARANCE_PROP => Some(Param::Base(self.appearance.into())),
                DURATION_PROP => Some(Param::Base(self.duration.into())),
                TO_PROP => Some(Param::Base(self.timeout.into())),
                //TODO:implement SND_CHANNEL_PROP => unimplemented!(),
                _ => None,
            },
            _ => None,
        }
    }
    fn set_inner(&mut self, _interface: &str, _prop: &str, _val: Variant) -> Option<String> {
        unimplemented!()
    }
}

impl Introspectable for Advertisement {
    fn introspectable_str(&self) -> String {
        let mut ret = String::new();
        ret.push_str(INTROSPECT_FMT_P1);
        ret.push_str(self.path.to_str().unwrap());
        ret.push_str(INTROSPECT_FMT_P2);
        ret.push_str(PROP_STR);
        ret.push_str(ADV_STR);
        ret.push_str(INTROSPECT_FMT_P3);
        ret
    }
}
