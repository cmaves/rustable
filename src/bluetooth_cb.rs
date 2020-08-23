use crate::Error;
use rustbus::{MessageType, message_builder::MarshalledMessage};
use std::rc::Rc;
use std::cell::Cell;

pub(crate) const POWER: &'static str = "Setting power";
pub(crate) const DISCOVERABLE: &'static str = "Setting discoverable";
pub(crate) fn set_power_cb(res: MarshalledMessage, (powered, on, err_str): (Rc<Cell<bool>>, bool, &'static str)) -> Result<(), Error>
{
				match res.typ {
                    MessageType::Reply => {
						powered.replace(on);
						Ok(())
					},
                    MessageType::Error => {
                        Err(Error::DbusReqErr(format!(
                            "{} call failed: {:?}",
                            err_str, res
                        )))
                    }
                    _ => unreachable!(),
             }
}
