use embedded_hal::digital::OutputPin;

use esp_hal::timer::systimer::SystemTimer;

use crate::{qq_alarm_queue::QQAlarmQueue, usb_writer::UsbWriter};

use super::Delay;



#[derive(Debug, Clone, Copy)]
pub struct StatusLedConfig {
    /// in system timer ticks
    pub boot_blink_duration: u64,
    pub boot_blink_count: usize,
}

enum StatusLedState {
    None,
    Booting {
        count: usize,
        delay: Delay,
    },
    UsbTimeoutMonitor(bool),
}

pub struct StatusLed<T> {
    led: T,
    boot_blink_duration: u64,
    boot_blink_count: usize,
    state: StatusLedState,
}

// TODO: maybe use peripherals for blinking instead of manual timing
impl<T> StatusLed<T> where T: OutputPin {
    // TODO: config defaults
    pub fn new(led: T, config: StatusLedConfig) -> Self {
        Self {
            led,
            boot_blink_duration: config.boot_blink_duration,
            boot_blink_count: 2 * config.boot_blink_count,
            state: StatusLedState::None,
        }
    }

    pub fn start(&mut self, qq: &mut impl QQAlarmQueue) {
        let delay = self.boot_set_led(qq, false);

        // TODO: assumes that currently state is `None` (for example two consecutive calls to `start`, will result in alarm (older) to be "leaked")
        self.state = StatusLedState::Booting {
            count: 0,
            delay,
        };
    }

    
    fn boot_set_led(&mut self, qq: &mut impl QQAlarmQueue, led_state: bool) -> Delay {
        self.led.set_state(led_state.into()).unwrap();

        let now = SystemTimer::now();
        let qq_alarm_id = qq.add(now + self.boot_blink_duration).unwrap();

        Delay::new(qq_alarm_id)
    }

    pub fn update(&mut self, usb_writer: &impl UsbWriter, qq: &mut impl QQAlarmQueue) -> bool {
        match self.state {
            StatusLedState::Booting { count, delay: Delay::Done } => {
                if count == self.boot_blink_count {
                    self.led.set_low().unwrap();

                    self.state = StatusLedState::UsbTimeoutMonitor(false);
                } else {
                    let delay = self.boot_set_led(qq, count % 2 == 0);
    
                    self.state = StatusLedState::Booting {
                        count: count + 1,
                        delay,
                    };
                }

                true
            },
            StatusLedState::UsbTimeoutMonitor(ref mut led_state) => {
                let timeout = usb_writer.is_timeouted();

                if timeout == *led_state {
                    return false;
                }

                *led_state = timeout;
                self.led.set_state(timeout.into()).unwrap();

                true
            },
            StatusLedState::None |
            StatusLedState::Booting { delay: Delay::Waiting { .. }, .. } => false,
        }
    }

    pub fn on_alarm(&mut self, qq_alarm_id: usize) -> bool {
        match &mut self.state {
            StatusLedState::Booting { delay, .. } => delay.on_alarm(qq_alarm_id),
            _ => false,
        }
    }
}