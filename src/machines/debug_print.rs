use core::fmt::Write;

use esp_hal::timer::systimer::SystemTimer;

use crate::qq_alarm_queue::QQAlarmQueue;
use super::Delay;



#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DebugPrintState {
    None,
    Waiting(Delay),
}

pub struct DebugPrint {
    state: DebugPrintState,
    delta: u64,
    tick_counter: usize,
    wakeup_counter: usize,
}

impl DebugPrint {
    pub fn new(delta: u64) -> DebugPrint {
        DebugPrint {
            state: DebugPrintState::None,
            delta,
            tick_counter: 0,
            wakeup_counter: 0,
        }
    }

    /// assumes that currently we are not waiting for alarm
    fn start_delay_unchecked(&mut self, qq: &mut impl QQAlarmQueue) {
        let wake_at = SystemTimer::now() + self.delta;
        let qq_alarm_id = qq.add(wake_at).unwrap();
        self.state = DebugPrintState::Waiting(Delay::new(qq_alarm_id));
    }

    pub fn start(&mut self, qq: &mut impl QQAlarmQueue) {
        if self.state == DebugPrintState::None {
            self.start_delay_unchecked(qq);
        }
    }

    pub fn wakeup(&mut self) {
        self.wakeup_counter += 1;
    }

    pub fn update(&mut self, qq: &mut impl QQAlarmQueue, usb_writer: &mut impl Write) -> bool {
        match self.state {
            DebugPrintState::Waiting(Delay::Done) => {
                let _ = writeln!(usb_writer, "DEBUG PRINT {}, wakeup count = {}", self.tick_counter, self.wakeup_counter);

                self.tick_counter += 1;

                self.start_delay_unchecked(qq);

                true                
            }
            _ => false,
        }
    }

    pub fn on_alarm(&mut self, qq_alarm_id: usize) -> bool {
        match &mut self.state {
            DebugPrintState::Waiting(delay) => delay.on_alarm(qq_alarm_id),
            _ => false,
        }
    }
}