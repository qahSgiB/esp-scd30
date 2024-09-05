#![no_std]
#![no_main]

#![feature(maybe_uninit_write_slice)]
#![feature(sync_unsafe_cell)]
#![feature(let_chains)]
#![feature(waker_getters)]



// use core::fmt::Write;


use core::{cell::RefCell, future::Future, pin::Pin, sync::atomic::AtomicU32, task::{Context, Poll, Waker}};

use bitflags::bitflags;
use esp_hal::{gpio::IO, peripherals::Peripherals, prelude::entry};
use esp_backtrace as _;


use interrupts::{i2c_interrupt_get, systimer_target0_interrupt_clear, systimer_target0_interrupt_get, systimer_target0_interrupt_get_and_clear, usb_interrupt_get, SystimerTartet0InterruptStatus};



mod ring_buffer;
mod interrupts;



// struct InterruptManager {
//     alarm0: Option<Waker>,
// }


struct Alarm0Future {

}

impl Alarm0Future {
    fn new() -> Alarm0Future {
        Alarm0Future {}
    }
}

impl Future for Alarm0Future {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let done = false; // [todo]

        if done {
            Poll::Ready(())
        } else {
            cx.waker().clone(); // [todo] register waker
            Poll::Pending
        }
    }
}



#[entry]
fn main() -> ! {
    // # init - common peripherals
    let peripherals = Peripherals::take();

    // let system = peripherals.SYSTEM.split();
    // let clocks = ClockControl::max(system.clock_control).freeze();

    // let io = IO::new(peripherals.GPIO, peripherals.IO_MUX);
    // let systimer = SystemTimer::new(peripherals.SYSTIMER);

    // # before loop
    let async_context = AsyncContext::new();

    // # loop
    loop {
        
    }
}