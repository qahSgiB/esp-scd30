use core::fmt::Write;

use esp_hal::{
    clock::Clocks,
    gpio::{Event, Input, InputPin, OutputOpenDrain, OutputPin, Pull},
    interrupt::Priority,
    peripheral::{Peripheral, PeripheralRef},
    peripherals::I2C0,
    timer::systimer::SystemTimer
};

use fugit::{RateExtU32, SecsDurationU32};

use crate::{
    interrupts::{self, GPIOInterruptStatus},
    qq_alarm_queue::QQAlarmQueue,
    sdc::{
        self,
        machines::{DelayedGet as SDCDelayedGet, DelayedGetError, Set as SDCSet, State as SDCState},
        SDCGetCommand,
        SDCSetCommand
    },
    pac_utils::i2c::{self as i2c_utils, I2CTransmissionError}
};

use super::{controller::Controller, Delay};



pub struct SDCSimpleMeasurmentConfig {
    pub delta: SecsDurationU32, // TODO: unit, constraints
    pub delayed_get_delta: Option<u64>, // TODO: unit
}

#[derive(Debug)]
pub(in crate::machines) enum SDCSimpleMeasurmentState {
    None,
    BootDelay(Delay),
    SetDelta(SDCSet),
    Start(SDCSet),
    WaitReady,
    Measurment(SDCDelayedGet),
    Error,
}


/// 1. boot delay
/// 2. set delta
/// 3. start
/// 4. wait
/// 5. is ready - if not go to 4.
/// 6. measurment - then go to 4.
/// 
/// Generic over sda, scl and ready pin types, so user can use either `GpioPin` or `AnyPin` or references to them.
pub struct SDCSimpleMeasurment<'a, 'b, 'c, 'd, SDA, SCL, RDY> {
    i2c: PeripheralRef<'a, I2C0>,
    scl_pin: OutputOpenDrain<'b, SCL>, // TODO: no need to hold this pins, remove and use phantom data for 'b, 'c, 'd lifetimes so this struct still acts like it holds this pins ??
    sda_pin: OutputOpenDrain<'c, SDA>,
    ready_pin: Input<'d, RDY>,
    delta: SecsDurationU32,
    delayed_get_delta: u64,
    state: SDCSimpleMeasurmentState,
}

impl<'a, 'b, 'c, 'd, SDA, SCL, RDY> SDCSimpleMeasurment<'a, 'b, 'c, 'd, SDA, SCL, RDY>
where
    SDA: OutputPin + InputPin,
    SCL: OutputPin + InputPin,
    RDY: InputPin,
{
    /// from sdc documentation: delay between i2c write and read should be at least 3ms
    /// default delay here is 5ms
    pub const DEFAULT_DELAYED_GET_DELTA: u64 = SystemTimer::TICKS_PER_SECOND / 200; // TODO: try lowering this


    pub fn new(
        i2c: impl Peripheral<P = I2C0> + 'a,
        scl_pin: impl Peripheral<P = SCL> + 'b,
        sda_pin: impl Peripheral<P = SDA> + 'c,
        ready_pin: impl Peripheral<P = RDY> + 'd,
        config: SDCSimpleMeasurmentConfig,
        clocks: &Clocks,
    ) -> Self {
        let mut i2c = i2c.into_ref();

        i2c_utils::setup(i2c.reborrow(), 50u32.kHz(), clocks);

        let (scl_pin, sda_pin) = i2c_utils::setup_pins(scl_pin, sda_pin);

        // TODO: if ready is already high interrupt is not fired
        let mut ready_pin = Input::new(ready_pin, Pull::None);
        ready_pin.listen(Event::RisingEdge);

        Self {
            i2c,
            scl_pin,
            sda_pin,
            ready_pin,
            delta: config.delta,
            delayed_get_delta: config.delayed_get_delta.unwrap_or(Self::DEFAULT_DELAYED_GET_DELTA),
            state: SDCSimpleMeasurmentState::None,
        }
    }

    /// This does not enable GPIO interrupt needed for ready pin, users should enable this interrupt themselves.
    pub fn enable_interrupt(&mut self) {
        interrupts::i2c_interrupt_enable(Some(Priority::Priority5));
    }

    pub fn start(&mut self, qq: &mut impl QQAlarmQueue) {
        let qq_alarm_id = qq.add(SystemTimer::now() + SystemTimer::TICKS_PER_SECOND * 5 / 2).unwrap();

        self.state = SDCSimpleMeasurmentState::BootDelay(Delay::new(qq_alarm_id));
    }

    fn after_error(&mut self, usb_writer: &mut impl Write, name_for_error: &str, error: I2CTransmissionError) -> bool {
        let _ = writeln!(usb_writer, "i2c error after {}: {:?}", name_for_error, error);
        self.state = SDCSimpleMeasurmentState::Error;

        true
    }

    pub fn update<const N: usize>(
        &mut self,
        usb_writer: &mut impl Write,
        qq: &mut impl QQAlarmQueue,
        controller: &mut Controller<N>
    ) -> bool {
        match &mut self.state {
            SDCSimpleMeasurmentState::BootDelay(Delay::Done) => {
                self.state = SDCSimpleMeasurmentState::SetDelta(SDCSet::start(self.i2c.reborrow(), SDCSetCommand::SetDelta { delta: self.delta }));
                true
            },
            SDCSimpleMeasurmentState::SetDelta(sdc_write) => {
                match sdc_write.update() {
                    SDCState::Done(Ok(())) => {
                        self.state = SDCSimpleMeasurmentState::Start(SDCSet::start(self.i2c.reborrow(), SDCSetCommand::Start { pressure: None }));
                        true
                    },
                    SDCState::Done(Err(err)) => self.after_error(usb_writer, "set delta", err),
                    SDCState::Active(did_something) => did_something,
                }
            },
            SDCSimpleMeasurmentState::Start(sdc_write) => {
                match sdc_write.update() {
                    SDCState::Done(Ok(())) => {
                        self.state = SDCSimpleMeasurmentState::WaitReady;
                        true
                    },
                    SDCState::Done(Err(err)) => self.after_error(usb_writer, "start", err),
                    SDCState::Active(did_something) => did_something,
                }
            },
            SDCSimpleMeasurmentState::WaitReady => {
                let pending_interrupts = interrupts::gpio_interrupt_get_and_clear(GPIOInterruptStatus::GPIO6);

                if !pending_interrupts.is_empty() {
                    self.state = SDCSimpleMeasurmentState::Measurment(SDCDelayedGet::start(self.i2c.reborrow(), SDCGetCommand::Measurment, self.delayed_get_delta));
                    true
                } else {
                    false
                }
            }
            SDCSimpleMeasurmentState::Measurment(sdc_delayed_get) => {
                match sdc_delayed_get.update(qq, self.i2c.reborrow()) {
                    SDCState::Done(Ok(())) => {
                        match sdc::read_response_measurment(self.i2c.reborrow()) {
                            Ok(measurment) => {
                                controller.on_measurment(measurment);
                                self.state = SDCSimpleMeasurmentState::WaitReady;
                            },
                            Err(err) => {
                                let _ = writeln!(usb_writer, "i2c error: measurment reading response ({:?})", err);
                                self.state = SDCSimpleMeasurmentState::Error;
                            }
                        }

                        true
                    },
                    SDCState::Done(Err(DelayedGetError::Write(err))) => self.after_error(usb_writer, "measurment write", err),
                    SDCState::Done(Err(DelayedGetError::Read(err))) => self.after_error(usb_writer, "measurment read", err),
                    SDCState::Active(active) => active,
                }
            }
            SDCSimpleMeasurmentState::None |
            SDCSimpleMeasurmentState::Error |
            SDCSimpleMeasurmentState::BootDelay(Delay::Waiting { .. }) => false,
        }
    }

    pub fn on_alarm(&mut self, qq_alarm_id: usize) -> bool {
        match &mut self.state {
            SDCSimpleMeasurmentState::BootDelay(delay) => delay.on_alarm(qq_alarm_id),
            SDCSimpleMeasurmentState::Measurment(sdc_delayed_get) => sdc_delayed_get.on_alarm(qq_alarm_id),
            _ => false
        }
    }
}