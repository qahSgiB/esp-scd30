pub mod controller;
pub mod debug_print;
pub mod sdc_simple_measurment;
pub mod status_led;
pub mod ir_nec_rx;



/// Helper state machine representing waiting for qq alarm
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delay {
    Waiting { qq_alarm_id: usize },
    Done,
}

impl Delay {
    pub fn new(qq_alarm_id: usize) -> Delay {
        Delay::Waiting { qq_alarm_id }
    }

    pub fn on_alarm(&mut self, qq_alarm_id: usize) -> bool {
        if let Delay::Waiting { qq_alarm_id: id } = self && *id == qq_alarm_id {
            *self = Delay::Done;
            true
        } else {
            false
        }
    }
}