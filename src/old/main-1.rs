#![no_std]
#![no_main]



use core::cell::RefCell;
use core::fmt::Write;


use esp_hal::{
    clock::ClockControl, gpio::{Event, Floating, Gpio6, Input}, interrupt::{self, Priority}, peripheral::Peripheral, peripherals::{Interrupt, Peripherals}, prelude::*, systimer::{Alarm, SystemTimer, Target}, IO
};
use esp_backtrace as _;

use critical_section::Mutex;


use sony_ir::{tx::SonyIREncoder, rx::{SonyIRDecoder, SonyIREvent}, SonyIRCommand};
use usb_writer::UsbWriter;



mod usb_writer;
mod rmt_tx;
mod sony_ir;




fn sony_ir_decoder_alarm_reset() {
    let target = SystemTimer::now() + SystemTimer::TICKS_PER_SECOND * 10 / 1000;

    critical_section::with(|cs| {
        let mut alarm0_lock = ALARM0.borrow_ref_mut(cs);
        let alarm0 = alarm0_lock.as_mut().unwrap();
        alarm0.set_target(target);
        alarm0.enable_interrupt(true);
    });
}



static IR_RECIEVER: Mutex<RefCell<Option<Gpio6<Input<Floating>>>>> = Mutex::new(RefCell::new(None));
static ALARM0: Mutex<RefCell<Option<Alarm<Target, 0>>>> = Mutex::new(RefCell::new(None));

static IR_EVENT: Mutex<RefCell<Option<SonyIREvent>>> = Mutex::new(RefCell::new(None));



#[entry]
fn main() -> ! {
    /* peripherals setup */
    let mut peripherals = Peripherals::take();

    // [todo] try to move this remote setup after split of SYSTEM
    sony_ir::tx::sony_ir_clock_config((&mut peripherals.SYSTEM).into_ref());

    let system = peripherals.SYSTEM.split();
    let _clocks = ClockControl::max(system.clock_control).freeze();
    let io = IO::new(peripherals.GPIO, peripherals.IO_MUX);
    let systimer = SystemTimer::new(peripherals.SYSTIMER);

    let mut ir_reciever = io.pins.gpio6.into_floating_input();
    ir_reciever.listen(Event::AnyEdge);

    critical_section::with(|cs| { IR_RECIEVER.borrow_ref_mut(cs).replace(ir_reciever); });

    let alarm0 = systimer.alarm0;
    alarm0.enable_interrupt(false);

    critical_section::with(|cs| { ALARM0.borrow_ref_mut(cs).replace(alarm0); });

    let mut led_green = io.pins.gpio11.into_push_pull_output();
    let mut led_yellow = io.pins.gpio10.into_push_pull_output();
    let mut led_red = io.pins.gpio8.into_push_pull_output();

    led_green.set_low().unwrap();
    led_yellow.set_low().unwrap();
    led_red.set_low().unwrap();

    let mut ir_transmitter = io.pins.gpio7.into_push_pull_output();
    ir_transmitter.set_low().unwrap();

    let mut usb_writer = UsbWriter::<1024>::with_timeout_treshold(peripherals.USB_DEVICE, SystemTimer::TICKS_PER_SECOND / 10_000); /* timeout_treshold: 0.1ms */

    let mut rmt = peripherals.RMT;

    // [todo] use only one ref
    rmt_tx::rmt_config((&mut rmt).into_ref(), true);
    sony_ir::tx::sony_ir_ch0_config((&mut rmt).into_ref(), &mut ir_transmitter);

    /* other setup */
    let mut ir_decoder = SonyIRDecoder::new();

    let mut ir_last_command = None;
    let mut ir_last_command_time = 0u64;

    let mut ir_encoder = SonyIREncoder::<32>::new();

    /* enable interrupts */
    interrupt::enable(Interrupt::GPIO, Priority::Priority2).unwrap();
    interrupt::enable(Interrupt::SYSTIMER_TARGET0, Priority::Priority3).unwrap();

    /* loop */
    // ir_encoder.send_non_immediatly(SonyIRCommand::V12 { address: 1, command: 117 }, 2).unwrap();
    // ir_encoder.send_non_immediatly(SonyIRCommand::V12 { address: 1, command: 101 }, 2).unwrap();
    // ir_encoder.send_non_immediatly(SonyIRCommand::V12 { address: 1, command: 51 }, 10).unwrap();
    // ir_encoder.send_non_immediatly(SonyIRCommand::V12 { address: 1, command: 101 }, 2).unwrap();
    // ir_encoder.send_non_immediatly(SonyIRCommand::V12 { address: 1, command: 116 }, 2).unwrap();
    // ir_encoder.send_non_immediatly(SonyIRCommand::V12 { address: 1, command: 51 }, 9).unwrap();
    // ir_encoder.send_non_immediatly(SonyIRCommand::V12 { address: 1, command: 101 }, 2).unwrap();
    // ir_encoder.send_non_immediatly(SonyIRCommand::V12 { address: 1, command: 52 }, 8).unwrap();
    // ir_encoder.send_non_immediatly(SonyIRCommand::V12 { address: 1, command: 117 }, 2).unwrap();
    // ir_encoder.send_non_immediatly(SonyIRCommand::V12 { address: 1, command: 101 }, 2).unwrap();

    let mut ppp = false;

    loop {
        /* update - usb */
        usb_writer.update();

        /* update - ir reciever */
        let ir_decoder_result = ir_encoder.update((&mut rmt).into_ref());
        if let Err(err) = ir_decoder_result {
            let _ = writeln!(usb_writer, "tx error : {:?}", err);
        }

        /* update - ir decoder */
        let ir_event = critical_section::with(|cs| { IR_EVENT.borrow_ref_mut(cs).take() });
        if let Some(SonyIREvent::Pulse(_)) = ir_event {
            sony_ir_decoder_alarm_reset();
        };

        let ir_action = ir_decoder.update(ir_event);
        let ir_command = match ir_action {
            Ok(command) => { command },
            Err(err) => {
                let _ = writeln!(usb_writer, "ir rx error : {:?}", err);
                None
            },
        };

        /* update - logic */
        if let Some(ir_command) = ir_command {
            let command = SonyIRCommand::from_raw(&ir_command);
            let command_time = SystemTimer::now();

            let command_repeat = ir_last_command.map_or(false, |ir_last_command| {
                ir_last_command == command && (command_time - ir_last_command_time) < SystemTimer::TICKS_PER_SECOND / 10 /* 100ms delay */
            });

            ir_last_command = Some(command);
            ir_last_command_time = command_time;

            let _ = writeln!(usb_writer, "{} | {} | {:?}", SystemTimer::now() / 16_000, command_repeat, command);

            match command {
                SonyIRCommand::V12 { address, command } => {
                    if address == 1 && !command_repeat {
                        match command {
                            0 => { led_red.toggle().unwrap(); },
                            1 => { led_yellow.toggle().unwrap(); },
                            2 => { led_green.toggle().unwrap(); },
                            18 => { ppp = true; }
                            _ => {},
                        }
                    }
                },
                _ => {}
            }
        }

        if SystemTimer::now() - ir_last_command_time > SystemTimer::TICKS_PER_SECOND * 2 {
            if ppp {
                ir_encoder.send_non_immediatly(SonyIRCommand::V12 { address: 1, command: 19 }, 23, None).unwrap();
                ppp = false;
            }
        }
    }
}


#[ram]
#[interrupt]
fn GPIO() {
    critical_section::with(|cs| {
        IR_RECIEVER.borrow_ref_mut(cs).as_mut().unwrap().clear_interrupt();
    });

    let now = SystemTimer::now();
    critical_section::with(|cs| { IR_EVENT.borrow_ref_mut(cs).replace(SonyIREvent::Pulse(now)); });
}

#[ram]
#[interrupt]
fn SYSTIMER_TARGET0() {
    critical_section::with(|cs| {
        ALARM0.borrow_ref_mut(cs).as_mut().unwrap().clear_interrupt();
        IR_EVENT.borrow_ref_mut(cs).replace(SonyIREvent::TimeOut);
    });
}