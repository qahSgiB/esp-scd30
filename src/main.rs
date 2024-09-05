#![no_std]
#![no_main]

#![feature(maybe_uninit_write_slice)]
#![feature(let_chains)]
#![feature(iter_array_chunks)]




use core::fmt::Write;


use esp_hal::{clock::ClockControl, gpio::{Io, Level, Output}, interrupt::Priority, peripherals::{Peripherals, SYSTEM}, prelude::*, system::SystemControl, timer::systimer::SystemTimer};
use esp_backtrace as _;

use fugit::ExtU32;


use qq_alarm_queue::DumbQQAlarmQueue;
use usb_writer::RingBufferUsbWriter;

use machines::{controller::Controller, debug_print::DebugPrint, ir_nec_rx::IrNecRx, sdc_simple_measurment::{SDCSimpleMeasurment, SDCSimpleMeasurmentConfig}, status_led::{StatusLed, StatusLedConfig}};



mod ring_buffer;
mod interrupts;
mod qq_alarm_queue;
mod usb_writer;
mod sdc;
mod machines;
mod pac_utils;

// mod sony_ir;



#[entry]
fn main() -> ! {
    // # init - common peripherals
    let peripherals = Peripherals::take();

    let system = SystemControl::new(peripherals.SYSTEM);
    let clocks = ClockControl::max(system.clock_control).freeze();

    let io = Io::new(peripherals.GPIO, peripherals.IO_MUX);
    let systimer = SystemTimer::new(peripherals.SYSTIMER);

    // # before loop
    let status_led = Output::new(io.pins.gpio7, Level::Low);

    let mut qq = DumbQQAlarmQueue::<8>::new(systimer.alarm0);
    let mut usb_writer = RingBufferUsbWriter::<4096>::new(peripherals.USB_DEVICE, None);

    let mut status_led = StatusLed::new(status_led, StatusLedConfig {
        boot_blink_duration: SystemTimer::TICKS_PER_SECOND / 10,
        boot_blink_count: 10,
    });
    let mut debug_print = DebugPrint::new(SystemTimer::TICKS_PER_SECOND);
    let mut sdc = SDCSimpleMeasurment::new(
        peripherals.I2C0,
        io.pins.gpio4,
        io.pins.gpio5,
        io.pins.gpio6,
        SDCSimpleMeasurmentConfig {
            delta: 10u32.secs(),
            delayed_get_delta: None,
        },
        &clocks,
    );
    // SAFETY: system is used only temporarily inside `IrNecRx::new` function, it is not stored in `ir_nec_rx` (cannot use `peripherals.SYSTEM` because it's already moved)
    let mut ir_nec_rx = IrNecRx::new(peripherals.RMT, io.pins.gpio10, unsafe { SYSTEM::steal() });
    let mut controller = Controller::<1024>::new();

    qq.enable_interrupt();
    usb_writer.enable_interrupt();
    sdc.enable_interrupt();
    interrupts::gpio_interrupt_enable(Some(Priority::Priority5));
    ir_nec_rx.enable_interrupt();

    // # start
    let _ = writeln!(usb_writer, "starting ...");

    status_led.start(&mut qq);
    debug_print.start(&mut qq);
    sdc.start(&mut qq);
    ir_nec_rx.start();

    let mut sleeping = false;

    // # loop
    loop {
        let mut did_something = false;

        did_something |= qq.update();

        if let Some(qq_pending_alarms) = qq.consume_pending() {
            qq_pending_alarms.for_each(|qq_alarm_id| {
                // if !usb_writer.on_alarm(qq_alarm_id) && !debug_print.on_alarm(qq_alarm_id) {
                if !status_led.on_alarm(qq_alarm_id) && !usb_writer.on_alarm(qq_alarm_id) && !sdc.on_alarm(qq_alarm_id) && !debug_print.on_alarm(qq_alarm_id) {
                    let _ = writeln!(usb_writer, "ajejeje ...");
                }
            });
        }

        did_something |= usb_writer.update(&mut qq);

        did_something |= status_led.update(&usb_writer, &mut qq);

        did_something |= debug_print.update(&mut qq, &mut usb_writer);

        did_something |= sdc.update(&mut usb_writer, &mut qq, &mut controller);

        did_something |= ir_nec_rx.update(&mut usb_writer);

        did_something |= controller.update(&mut usb_writer);

        // critcal section disables interrupts
        // TODO: critical section works ??? go to sleep and enable interrupts in one cycle
        // TODO: interrupts
        // `systimer_target0` - always awaited
        // `usb` - managed (on/off) by usb task, when on always awaited
        // `i2c` - managed by sdc i2c task
        //         always on and only selected relevant subinterrupts enabled
        //         (not always awaited, but) when interrupt can happen sdc task is always waiting on it
        // `gpio` - not working, awaited when not needed (maybe ???)
        critical_section::with(|_cs| {
            let no_interrupts = interrupts::systimer_target0_interrupt_get().is_empty()
                && interrupts::usb_interrupt_get().is_empty()
                && interrupts::i2c_interrupt_get().is_empty()
                && interrupts::gpio_interrupt_get().is_empty()
                && interrupts::rmt_interrupt_get().is_empty();

            if no_interrupts && !did_something {
                sleeping = true;
            } else {
                if sleeping {
                    debug_print.wakeup();
                }

                sleeping = false;
            }
        })
    }
}