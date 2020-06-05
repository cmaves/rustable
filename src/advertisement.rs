use crate::interfaces::*;
use crate::*;

use std::time::Instant;

pub struct Advertisement {
    pub typ: AdType,
    pub service_uuids: Vec<UUID>,
    // pub manu_data: HashMap<String, ()>
    pub solicit_uuids: Vec<UUID>,
    pub timeout: u16,
    pub duration: u16,
    pub(crate) index: u16,
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
            timeout: 180,
            timeout_start: now,
            duration: 180,
            duration_start: now,
            appearance: 0,
            localname,
            index: 0,
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
impl AdType {
    pub fn to_str(&self) -> &'static str {
        match self {
            AdType::Peripheral => "peripheral",
            AdType::Broadcast => "broadcast",
        }
    }
}

impl<'a, 'b> Properties<'a, 'b> for Advertisement {
    const INTERFACES: &'static [(&'static str, &'static [&'static str])] = &[LEAD_IF, PROP_IF];
    fn get_inner(&mut self, interface: &str, prop: &str) -> Option<Param<'a, 'b>> {
        match interface {
            LEAD_IF_STR => match prop {
                TYPE_PROP => unimplemented!(),
                SERV_UUIDS_PROP => unimplemented!(),
                MANU_DATA_PROP => unimplemented!(),
                SOLICIT_UUIDS_PROP => unimplemented!(),
                SERV_DATA_PROP => unimplemented!(),
                DATA_PROP => unimplemented!(),
                DISCOVERABLE_PROP => unimplemented!(),
                DISCOVERABLE_TO_PROP => unimplemented!(),
                INCLUDES_PROP => unimplemented!(),
                LOCAL_NAME_PROP => unimplemented!(),
                APPEARANCE_PROP => unimplemented!(),
                DURATION_PROP => unimplemented!(),
                TO_PROP => unimplemented!(),
                SND_CHANNEL_PROP => unimplemented!(),
                _ => None,
            },
            _ => None,
        }
    }
    fn set_inner(&mut self, interface: &str, prop: &str, val: &params::Variant) -> Option<String> {
        unimplemented!()
    }
}
