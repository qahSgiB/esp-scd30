// Programming model (how are the instructions run, order, ...)
// Programming model of esp32c6 (microcontroller) is lot simpler than model of traditional process running inside a OS.
// On esp32c6 in this code we assume that there is single main "thread", corresponding to the function marked with `entry` attribute (in this crate `fn main`).
// Additionally there are interrupts which can interrupt and temporarily suspend the main thread.
// Nested interrupts are disallowed.
//
// This means that access to `static mut` can be done safely without mutexes and things like that if done with care.
//
// For example if we have `static mut` which is accessed only inside interrupts than taking nonmutable or mutable ref inside given interrupts is completly safe.
// Interrupt cannot be interrupted which means that interrupt cannot be suspended so datarace by being suspended by some other thread and the other thread creating aliasing ref is not possible.
// Also there cannot be ref to given `static mut` cannot exist in thread which was running before interrupt happened.
// This is because interrupt couldn't be running before (nested interrupts are disallowed), so the main thread must have been running and the presumption was that `static mut` is used only in interrupts code.
//
// Safe access is possible also for `static mut` which is used inside main and also inside interrupts.
// In the interrupts we can directly access this `static mut` and in the main thread we need to wrap the access by critacal section.
// Crital section in sense that we need to disable interrupts for the duration of access to `static mut`.
// Reasoning why access to `static mut` inside interrupts as described here is safe is equivalent to the reasonging in the paragraph above but with the assumption that access inside main is wrapped in critical section.
// [todo] why is access inside main safe
//
// In above text is used term `static mut`, but inside this crate for variables which need to be static and mutable is rather used unmutable static with `SyncUnsafeCell`.
// `static VARIABLE: SyncUnsafeCell<T>` is used instead of `static mut VARIABLE: T`, but this should be essentially same.
// Maybe version with `SyncUnsafeCell` is little bit safer and `static mut` are to be deprecated probably.
//
// [todo] discuss main with interrupts disabled (during initialization for example)



#![no_std]
#![no_main]

#![feature(maybe_uninit_write_slice)]
#![feature(sync_unsafe_cell)]



use core::{cell::SyncUnsafeCell, cmp, fmt::Write, iter, mem::MaybeUninit, sync::atomic::{AtomicBool, AtomicU32, Ordering}};

use bitflags::bitflags;
use esp_hal::{
    clock::{ClockControl, Clocks}, gpio::{Gpio20, InputSignal, Output, OutputSignal, PushPull}, interrupt::{self, Priority}, peripheral::{Peripheral, PeripheralRef}, peripherals::{Interrupt, Peripherals, I2C0, USB_DEVICE}, prelude::*, systimer::{Alarm, SystemTimer, Target}, IO
};
use esp_backtrace as _;

use fugit::HertzU32;


use ring_buffer::{RingBuffer, RingBufferError};



mod ring_buffer;



// [todo] statics accessed from different threads may have problem with memory ordering and "non volatile" writes (caching)
// # usb
static USB_BUFFER: SyncUnsafeCell<RingBuffer<u8, 1024>> = SyncUnsafeCell::new(RingBuffer::new());
static USB_TIMEOUT: AtomicBool = AtomicBool::new(false); // [todo] what does Atomic really does on esp

const USB_TIMEOUT_THRESHOLD: u64 = SystemTimer::TICKS_PER_SECOND / 1_000;

// # i2c
static I2C_PENDING_INTERUPTS: AtomicU32 = AtomicU32::new(I2CInterruptsStatus::empty().bits());

// # alarm queue
static QQ_ALARM_QUEUE: SyncUnsafeCell<QQAlarmQueue<10>> = SyncUnsafeCell::new(QQAlarmQueue::new());
static QQ_ALARM_ANY_PENDING: AtomicBool = AtomicBool::new(false);

// # global peripherals
// all these statics are zero sized
static USB: SyncUnsafeCell<MaybeUninit<USB_DEVICE>> = SyncUnsafeCell::new(MaybeUninit::uninit());
static I2C: SyncUnsafeCell<MaybeUninit<I2C0>> = SyncUnsafeCell::new(MaybeUninit::uninit());
static ALARM0: SyncUnsafeCell<MaybeUninit<Alarm<Target, 0>>> = SyncUnsafeCell::new(MaybeUninit::uninit());
static DEBUG_LED: SyncUnsafeCell<MaybeUninit<Gpio20<Output<PushPull>>>> = SyncUnsafeCell::new(MaybeUninit::uninit());



bitflags! {
    pub struct I2CInterruptsStatus: u32 {
        const ARBITRATION_LOST = 1 << 5;
        const TRANSACTION_COMPLETE = 1 << 7;
        const TIME_OUT = 1 << 8;
        const NACK = 1 << 10;
        const SCL_ST_TIME_OUT = 1 << 13;
        const SCL_MAIN_ST_TIME_OUT = 1 << 14;
        const ERROR = I2CInterruptsStatus::ARBITRATION_LOST.bits() |
                      I2CInterruptsStatus::TIME_OUT.bits() |
                      I2CInterruptsStatus::NACK.bits() |
                      I2CInterruptsStatus::SCL_ST_TIME_OUT.bits() |
                      I2CInterruptsStatus::SCL_MAIN_ST_TIME_OUT.bits();
    }
}



struct QQAlarmQueue<const N: usize> {
    queue: [Option<QQAlarm>; N],
    next_wakeup: Option<u64>,
    next_id: usize,
}

impl<const N: usize> QQAlarmQueue<N> {
    const fn new() -> Self {
        QQAlarmQueue {
            queue: [None; N],
            next_wakeup: None,
            next_id: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QQAlarmState { Waiting, Pending }

#[derive(Debug, Clone, Copy)]
struct QQAlarmMetatdata {
    id: usize,
    wake_at: u64,
}

#[derive(Debug, Clone, Copy)]
struct QQAlarm {
    state: QQAlarmState,
    m: QQAlarmMetatdata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QQAlarmError {
    QueueFull,
    IdNotFound,
}


/// # Safety
/// 
/// This function accesses `QQ_ALARM_QUEUE`, user is responsible for checking that no data races happens on it.
/// 
/// This function accesses `ALARM0`, user is responsible for checking that no data races happens on this variable.
/// `ALARM0` may also be unitialized, it is up to user to ensure that it is already initialized.
/// 
/// Basicly safety conditions are equivalent to checking:
///  - (mut ref to static) for `QQ_ALARM_QUEUE`
///  - (mut ref to static) for `ALARM0`
///  - (MaybeUninit::assume_init_mut) for `ALARM0`
unsafe fn qq_alarm_add(wake_at: u64) -> Result<usize, QQAlarmError> {
    // assuming wake_at is less than now (if it is not it is ok alarm will cause interrupt instantly)

    let alarm_queue_ptr = QQ_ALARM_QUEUE.get();
    // [safety] (mut ref to static) checked by user
    let alarm_queue = unsafe { &mut *alarm_queue_ptr };

    let alarm_ptr = ALARM0.get();
    // [safety] (mut ref to static) checked by user
    // [safety] (MaybeUninit::assume_init_mut) checked by user
    let alarm = unsafe { (&mut *alarm_ptr).assume_init_mut() };

    let id = alarm_queue.next_id;
    alarm_queue.next_id += 1;

    let empty_alarm = alarm_queue.queue.iter_mut().find(|alarm| alarm.is_none()).ok_or(QQAlarmError::QueueFull)?;
    *empty_alarm = Some(QQAlarm {
        state: QQAlarmState::Waiting,
        m: QQAlarmMetatdata {
            id,
            wake_at,
        }
    });

    let set_target = match alarm_queue.next_wakeup {
        Some(next_wakeup) => wake_at < next_wakeup,
        None => {
            alarm.clear_interrupt();
            alarm.enable_interrupt(true);
            true
        }
    };

    if set_target {
        alarm.set_target(wake_at);
        alarm_queue.next_wakeup = Some(wake_at);
    }

    Ok(id)
}

/// # Safety
/// 
/// This function accesses `QQ_ALARM_QUEUE`, user is responsible for checking that no data races happens on it.
/// 
/// This function accesses `ALARM0`, user is responsible for checking that no data races happens on this variable.
/// `ALARM0` may also be unitialized, it is up to user to ensure that it is already initialized.
/// 
/// Basicly safety conditions are equivalent to checking:
///  - (mut ref to static) for `QQ_ALARM_QUEUE`
///  - (mut ref to static) for `ALARM0`
///  - (MaybeUninit::assume_init_mut) for `ALARM0`
unsafe fn qq_alarm_remove(id: usize) -> Result<(), QQAlarmError> {
    let alarm_queue_ptr = QQ_ALARM_QUEUE.get();
    // [safety] (mut ref to static) checked by user
    let alarm_queue = unsafe { &mut *alarm_queue_ptr };

    let alarm_ptr = ALARM0.get();
    // [safety] (mut ref to static) checked by user
    // [safety] (MaybeUninit::assume_init_mut) checked by user
    let alarm = unsafe { (&mut *alarm_ptr).assume_init_mut() };

    let mut id_found = false;
    
    let mut min_wake_at = None;
    let mut any_pending = false;

    for qq_alarm_opt in alarm_queue.queue.iter_mut() {
        match qq_alarm_opt {
            None => {},
            Some(qq_alarm) => {
                if qq_alarm.m.id == id {
                    id_found = true;
                    *qq_alarm_opt = None
                } else {
                    match qq_alarm.state {
                        QQAlarmState::Waiting => {
                            let wake_at = qq_alarm.m.wake_at;
                            min_wake_at = Some(min_wake_at.map_or(wake_at, |min_wake_at| cmp::min(min_wake_at, wake_at)));
                        },
                        QQAlarmState::Pending => {
                            any_pending = true;
                        }
                    }
                }
            },
        }
    }

    if !id_found {
        return Err(QQAlarmError::IdNotFound);
    }

    match min_wake_at {
        None => {
            if alarm_queue.next_wakeup.is_some() {
                alarm.enable_interrupt(false);
                alarm_queue.next_wakeup = None;
            }
        },
        Some(min_wake_at) => {
            // `alarm_queue.next_wakeup` cannot be `None` because we found some waiting alarms
            if min_wake_at != alarm_queue.next_wakeup.unwrap() {
                alarm.set_target(min_wake_at);
                alarm_queue.next_wakeup = Some(min_wake_at);
            }
        },
    }

    // update to `QQ_ALARM_ANY_PENDING` is needed when deleted alarm was pending alarm and all other alarms were not pending (`QQ_ALARM_ANY_PENDING` is changed from `true` to `false`)
    QQ_ALARM_ANY_PENDING.store(any_pending, Ordering::Release);

    Ok(())
}

fn qq_alarm_any_pending() -> bool {
    QQ_ALARM_ANY_PENDING.load(Ordering::Acquire)
}

/// # Safety
/// 
/// This function creates mut ref to `QQ_ALARM_QUEUE` and this ref is valid (used) until the iterator created by this function is dropped.
/// User is responsible for checking that no data races happens on `QQ_ALARM_QUEUE`.
/// 
/// Basicly safety conditions are equivalent to checking:
///  - (mut ref to static) for `QQ_ALARM_QUEUE` (with lifetime same as lifetime of returned object)
/// 
unsafe fn qq_alarm_consume_pending() -> impl Iterator<Item = usize> {
    let alarm_queue_ptr = QQ_ALARM_QUEUE.get();
    // [safety] (mut ref to static) checked by user
    let alarm_queue = unsafe { &mut *alarm_queue_ptr };

    // [todo] side effects in iterator
    alarm_queue.queue.iter_mut()
        .map(|qq_alarm_opt| {
            match qq_alarm_opt {
                None => None,
                Some(qq_alarm) => {
                    if qq_alarm.state == QQAlarmState::Pending {
                        let id = qq_alarm.m.id;
                        *qq_alarm_opt = None;
                        Some(id)
                    } else {
                        None
                    }
                }
            }
        })
        .chain(iter::once_with(|| {
            // sets `QQ_ALARM_ANY_PENDING` to false after all pending alarms are set to `None`
            QQ_ALARM_ANY_PENDING.store(false, Ordering::Release);
            None
        }))
        .filter_map(|id| id)
}



// [todo] lifetimes
struct TemporaryUsbBufferWriter<'a, 'b, const N: usize, const ALARM_CHANNEL: u8> {
    usb_buffer: &'a mut RingBuffer<u8, N>,
    usb: PeripheralRef<'b, USB_DEVICE>,
}

impl<'a, 'b, const N: usize, const ALARM_CHANNEL: u8> TemporaryUsbBufferWriter<'a, 'b, N, ALARM_CHANNEL> {
    fn new(usb_buffer: &'a mut RingBuffer<u8, N>, usb: impl Peripheral<P = USB_DEVICE> + 'b) -> Self {
        Self { usb_buffer, usb: usb.into_ref() }
    }

    fn write_with_timeout(&mut self, bytes: &[u8]) -> Result<(), RingBufferError> {
        let empty_before = self.usb_buffer.len() == 0;
    
        let r = self.usb_buffer.extend_from_slice(bytes);
    
        if empty_before {
            // usb_reg.int_clr().write(|w| w.serial_in_empty_int_clr().set_bit());
            self.usb.int_ena().modify(|_, w| w.serial_in_empty_int_ena().set_bit());
    
            // self.alarm.enable_interrupt(true);
            // self.alarm.clear_interrupt();
            // self.alarm.set_target(SystemTimer::now() + USB_TIMEOUT_THRESHOLD);
        }
    
        r
    }
}

impl<'a, 'b> TemporaryUsbBufferWriter<'a, 'b, 1024, 0> {
    /// # Safety
    /// 
    /// This function creates mut ref to `USB_BUFFER` and this ref is valid (used) until the `UsbBufferWriter` created by this function is dropped.
    /// User is responsible for checking that no data races happens on `USB_BUFFER`.
    /// 
    /// This function also creates PeripheralRef to `USB_DEVICE`, same safety conditions as for `USB_BUFFER` apply here.
    /// `USB_DEVICE` may also be unitialized, it is up to user to ensure that it is already initialized.
    /// 
    /// Basicly safety conditions are equivalent to checking:
    ///  - (mut ref to static) for `USB_BUFFER` (with lifetime same as lifetime of returned object)
    ///  - (mut ref to static) for `USB` (with lifetime same as lifetime of returned object)
    ///  - (MaybeUninit::assume_init_mut) for `USB`
    unsafe fn from_static() -> Self {
        let usb_buffer_ptr = USB_BUFFER.get();
        // [safety] (mut ref to static) checked by user
        let usb_buffer = unsafe { &mut *usb_buffer_ptr };

        let usb_ptr = USB.get();
        // [safety] (mut ref to static) checked by user
        // [safety] (MaybeUninit::assume_init_mut) checked by user
        let usb = unsafe { (&mut *usb_ptr).assume_init_mut() };

        TemporaryUsbBufferWriter { usb_buffer, usb: usb.into_ref() }
    }
}

impl<'a, 'b, const N: usize, const ALARM_CHANNEL: u8> Write for TemporaryUsbBufferWriter<'a, 'b, N, ALARM_CHANNEL> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_with_timeout(s.as_bytes()).map_err(|_| core::fmt::Error)
    }
}


/// # Safety
/// Same as `UsbBufferWriter::from_static`
unsafe fn uw() -> TemporaryUsbBufferWriter<'static, 'static, 1024, 0> {
    TemporaryUsbBufferWriter::from_static()
}


// macros "copied" from esp-println
// only unsafe operation inside `print` and others is call to `uw`
// <>_i versions ignore result returned from `write` macro (currently only possible error is buffer overflow which may be desired to ignore)

/// # Safety
/// Same as `uw`
macro_rules! print {
    ($($arg:tt)*) => {{
        {
            use core::fmt::Write;
            write!($crate::uw(), $($arg)*)
        }
    }};
}

/// # Safety
/// Same as `uw`
macro_rules! println {
    ($($arg:tt)*) => {{
        {
            use core::fmt::Write;
            writeln!($crate::uw(), $($arg)*)
        }
    }};
}

/// # Safety
/// Same as `uw`
macro_rules! print_i {
    ($($arg:tt)*) => {{
        {
            use core::fmt::Write;
            let _ = write!($crate::uw(), $($arg)*);
        }
    }};
}

/// # Safety
/// Same as `uw`
macro_rules! println_i {
    ($($arg:tt)*) => {{
        {
            use core::fmt::Write;
            let _ = writeln!($crate::uw(), $($arg)*);
        }
    }};
}



#[derive(Debug, Clone, Copy)]
pub enum I2CCommand {
    Write {
        ack_ckeck: bool,
        ack_exp: bool,
        len: u8,
    },
    Read {
        ack: bool,
        len: u8,
    },
    Start,
    Stop, // proper finish
    End, // finish but hold the line (repeated start) ???
}

impl From<I2CCommand> for u16 {
    fn from(value: I2CCommand) -> u16 {
        match value {
            I2CCommand::Write { ack_ckeck, ack_exp, len } => (1 << 11) | ((ack_exp as u16) << 9) | ((ack_ckeck as u16) << 8) | (len as u16),
            I2CCommand::Read { ack, len } => (3 << 11) | ((ack as u16) << 10) | (len as u16),
            I2CCommand::Start => 6 << 11,
            I2CCommand::Stop => 2 << 11,
            I2CCommand::End => 4 << 11,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum I2CTransactionError {
    ArbitrationLost,
    Nack,
    TimeOutScl,
    TimeOutMainFsm,
    TimeOutFsm,
}


fn i2c_simple_setup(mut i2c: PeripheralRef<I2C0>, freq: HertzU32, clocks: &Clocks) {
    i2c.setup(freq, clocks, None); // [todo] look into this

    i2c.fifo_conf().modify(|_, w| {
        w.nonfifo_en().clear_bit()
         .fifo_prt_en().clear_bit()
    });

    i2c.int_ena().modify(|_, w| {
        w.trans_complete_int_ena().set_bit()
         .arbitration_lost_int_ena().set_bit()
         .nack_int_ena().set_bit()
         .time_out_int_ena().set_bit()
         .scl_main_st_to_int_ena().set_bit()
         .scl_st_to_int_ena().set_bit()
    });
}

fn i2c_reset_fifo(i2c: PeripheralRef<I2C0>) {
    i2c.fifo_conf().modify(|_, w| {
        w.tx_fifo_rst().set_bit()
         .rx_fifo_rst().set_bit()
    });

    i2c.fifo_conf().modify(|_, w| {
        w.tx_fifo_rst().clear_bit()
         .rx_fifo_rst().clear_bit()
    });
}

// [todo] should this be unsafe?
/// # Peripheral safety
/// 
/// `bytes.len() <= 31` - exp32-c6 I2C fifo has maximum capacity of 32 bytes and one byte is used for the address
fn i2c_prepare_simple_write_unchecked(i2c: PeripheralRef<I2C0>, address: u8, bytes: &[u8]) {
    let commands = [
        I2CCommand::Start,
        I2CCommand::Write { ack_ckeck: true, ack_exp: false, len: (bytes.len() + 1) as u8 },
        I2CCommand::Stop,
    ];
    i2c.comd_iter().zip(commands.into_iter()).for_each(|(cmd_reg, cmd)| cmd_reg.write(|w| w.command().variant(cmd.into())));

    i2c.data().write(|w| w.fifo_rdata().variant((address << 1) | 0));
    bytes.into_iter().for_each(|byte| i2c.data().write(|w| w.fifo_rdata().variant(*byte)));
}

/// # Peripheral safety
/// 
/// `len <= 32` - exp32-c6 I2C fifo has maximum capacity of 32 bytes
fn i2c_prepare_simple_read_unchecked(i2c: PeripheralRef<I2C0>, address: u8, len: u8) {
    let commands = [
        I2CCommand::Start,
        I2CCommand::Write { ack_ckeck: true, ack_exp: false, len: 1 },
        I2CCommand::Read { ack: false, len: len - 1 },
        I2CCommand::Read { ack: true, len: 1 },
        I2CCommand::Stop,
    ];
    i2c.comd_iter().zip(commands.into_iter()).for_each(|(cmd_reg, cmd)| cmd_reg.write(|w| w.command().variant(cmd.into())));

    i2c.data().write(|w| w.fifo_rdata().variant((address << 1) | 1));
}

fn i2c_start(i2c: PeripheralRef<I2C0>) {
    i2c.ctr().modify(|_, w| w.trans_start().set_bit());
}



#[entry]
fn main() -> ! {
    // # init - common peripherals
    let peripherals = Peripherals::take();

    let system = peripherals.SYSTEM.split();
    let clocks = ClockControl::max(system.clock_control).freeze();

    let mut io = IO::new(peripherals.GPIO, peripherals.IO_MUX);
    let systimer = SystemTimer::new(peripherals.SYSTIMER);

    // # init - usb
    {
        let usb_ptr = USB.get();
        // [safety] (mut ref to static) inside main + interrupts disabled
        let usb = unsafe { &mut *usb_ptr };
        // [safety] (memory leak) `usb` is definitly not initialized so there is no need to drop inner data before replacing it
        usb.write(peripherals.USB_DEVICE);
    } // explicit lifetime for `usb`

    // # init - alarm0
    let alarm0 = systimer.alarm0;

    {
        let alarm0_ptr = ALARM0.get();
        // [safety] (mut ref to static) inside main + interrupts disabled
        let alarm0_g = unsafe { &mut *alarm0_ptr };
        // [safety] (memory leak) `alarm0_g` is definitly not initialized so there is no need to drop inner data before replacing it
        alarm0_g.write(alarm0);
    } // explicit lifetime for `alarm0_g`

    // # init - debug led (global)
    let mut led = io.pins.gpio20.into_push_pull_output();
    led.set_high().unwrap();

    {
        let debug_led_ptr = DEBUG_LED.get();
        // [safety] (mut ref to static) inside main + interrupts disabled
        let debug_led = unsafe { &mut *debug_led_ptr };
        // [safety] (memory leak) `debug_led` is definitly not initialized so there is no need to drop inner data before replacing it
        debug_led.write(led);
    } // explicit lifetime for `debug_led`

    // # init - i2c
    io.pins.gpio4
        .set_to_open_drain_output()
        .enable_input(true)
        .internal_pull_up(false)
        .connect_peripheral_to_output(OutputSignal::I2CEXT0_SCL)
        .connect_input_to_peripheral(InputSignal::I2CEXT0_SCL);

    io.pins.gpio5
        .set_to_open_drain_output()
        .enable_input(true)
        .internal_pull_up(false)
        .connect_peripheral_to_output(OutputSignal::I2CEXT0_SDA)
        .connect_input_to_peripheral(InputSignal::I2CEXT0_SDA);

    let mut i2c = peripherals.I2C0;

    i2c_simple_setup((&mut i2c).into_ref(), 50u32.kHz(), &clocks);

    {
        let i2c_ptr = I2C.get();
        // [safety] (mut ref to static) inside main + interrupts disabled
        let i2c_g = unsafe { &mut *i2c_ptr };
        // [safety] (memory leak) `i2c` is definitly not initialized so there is no need to drop inner data before replacing it
        i2c_g.write(i2c);
    } // explicit lifetime for `i2c`

    // # init - enable interrupts
    interrupt::enable(Interrupt::USB_DEVICE, Priority::Priority3).unwrap();
    interrupt::enable(Interrupt::SYSTIMER_TARGET0, Priority::Priority2).unwrap();
    interrupt::enable(Interrupt::I2C_EXT0, Priority::Priority2).unwrap();

    // # before loop
    enum I2CDebugState { DoWrite, WaitWriteDone, AfterWrite, DoRead, WaitReadDone, AfterRead, Done, Error }

    let mut i2c_debug_state = I2CDebugState::DoWrite;

    let next_print = SystemTimer::now() + SystemTimer::TICKS_PER_SECOND * 10;

    let mut debug_print_id_1 = critical_section::with(|_cs| {
        // [safety] (mut ref to static, by access to `QQ_ALARM_QUEUE`, `ALARM0`) inside main + critical section
        // [safety] (MaybeUninit::assume_init_mut, by access to `ALARM0`) already initialized
        unsafe { qq_alarm_add(next_print) }
    }).unwrap();

    let next_print = SystemTimer::now() + SystemTimer::TICKS_PER_SECOND * 5;

    let mut debug_print_id_2 = critical_section::with(|_cs| {
        // [safety] (mut ref to static, by access to `QQ_ALARM_QUEUE`, `ALARM0`) inside main + critical section
        // [safety] (MaybeUninit::assume_init_mut, by access to `ALARM0`) already initialized
        unsafe { qq_alarm_add(next_print) }
    }).unwrap();

    // # loop
    loop {
        let i2c_pending_interrupts = I2CInterruptsStatus::from_bits_retain(I2C_PENDING_INTERUPTS.load(Ordering::Relaxed));

        if !i2c_pending_interrupts.is_empty() {
            if i2c_pending_interrupts.intersects(I2CInterruptsStatus::ERROR) {
                i2c_debug_state = I2CDebugState::Error;
            } else {

            }
        }

        // # i2c debug
        match i2c_debug_state {
            I2CDebugState::DoWrite => {
                {
                    let i2c_ptr = I2C.get();
                    // [safety] (mut ref to static) inside main + only interrupts related register of `I2C` are used in interrupts and here we are not using these interrupt related registers, so no data races can happen because no data is really shared
                    // [safety] (MaybeUninit::assume_init_mut) `I2C` is initialized
                    let i2c = unsafe { (&mut *i2c_ptr).assume_init_mut() };

                    i2c_reset_fifo(i2c.into_ref());

                    // [peripheral safety] `bytes.len() <= 31`
                    i2c_prepare_simple_write_unchecked(i2c.into_ref(), 0x61, &[0xD1, 0x00]); // read firmware version
                    // i2c_prepare_simple_write_unchecked(i2c.into_ref(), 0x61, &[0x51, 0x02]);
                    // i2c_prepare_simple_write_unchecked(i2c.into_ref(), 0x61, &[0x53, 0x06]);
                    // i2c_prepare_simple_write_unchecked(i2c.into_ref(), 0x61, &[0x02, 0x02]);

                    i2c_start(i2c.into_ref());
                }

                let now_ms = SystemTimer::now() / (SystemTimer::TICKS_PER_SECOND / 1000);

                critical_section::with(|_cs| {
                    // [safety] (mut ref to static, by access to `USB_BUFFER`, `USB`) inside main + critical section
                    // [safety] (MaybeUninit::assume_init_mut, by access to `USB`) already initialized
                    unsafe { println_i!("i2c debug write start # {}", now_ms) };
                });

                i2c_debug_state = I2CDebugState::WaitWriteDone;
            },
            I2CDebugState::WaitWriteDone => {
                if I2C_SIMPLE_TRANSACTION_DONE.load(Ordering::Acquire) {
                    I2C_SIMPLE_TRANSACTION_DONE.store(false, Ordering::Relaxed);

                    let write_result = critical_section::with(|_cs| {
                        let i2c_simple_transaction_result_ptr = I2C_SIMPLE_TRANSACTION_RESULT.get();
                        // [safety] (mut ref to static) inside main + critical section
                        *unsafe { &mut *i2c_simple_transaction_result_ptr }
                    });

                    let now_ms = SystemTimer::now() / (SystemTimer::TICKS_PER_SECOND / 1000);

                    critical_section::with(|_cs| {
                        // [safety] (mut ref to static, by access to `USB_BUFFER`, `USB`) inside main + critical section
                        // [safety] (MaybeUninit::assume_init_mut, by access to `USB`) already initialized
                        unsafe { println_i!("i2c debug write done (result: {:?}) # {}", write_result, now_ms) };
                    });

                    i2c_debug_state = I2CDebugState::DoRead;
                }
            },
            I2CDebugState::DoRead => {
                {
                    let i2c_ptr = I2C.get();
                    // [safety] (mut ref to static) inside main + only interrupts related register of `I2C` are used in interrupts and here we are not using these interrupt related registers, so no data races can happen because no data is really shared
                    // [safety] (MaybeUninit::assume_init_mut) `I2C` is initialized
                    let i2c = unsafe { (&mut *i2c_ptr).assume_init_mut() };

                    i2c_reset_fifo(i2c.into_ref());

                    // [peripheral safety] `len <= 32`
                    i2c_prepare_simple_read_unchecked(i2c.into_ref(), 0x61, 3);

                    i2c_start(i2c.into_ref());
                }

                let now_ms = SystemTimer::now() / (SystemTimer::TICKS_PER_SECOND / 1000);

                critical_section::with(|_cs| {
                    // [safety] (mut ref to static, by access to `USB_BUFFER`, `USB`) inside main + critical section
                    // [safety] (MaybeUninit::assume_init_mut, by access to `USB`) already initialized
                    unsafe { println_i!("i2c debug read start # {}", now_ms) };
                });

                i2c_debug_state = I2CDebugState::WaitReadDone;
            },
            I2CDebugState::WaitReadDone => {
                if I2C_SIMPLE_TRANSACTION_DONE.load(Ordering::Acquire) {
                    I2C_SIMPLE_TRANSACTION_DONE.store(false, Ordering::Relaxed);

                    let read_result = critical_section::with(|_cs| {
                        let i2c_simple_transaction_result_ptr = I2C_SIMPLE_TRANSACTION_RESULT.get();
                        // [safety] (mut ref to static) inside main + critical section
                        *unsafe { &mut *i2c_simple_transaction_result_ptr }
                    });

                    let now_ms = SystemTimer::now() / (SystemTimer::TICKS_PER_SECOND / 1000);

                    critical_section::with(|_cs| {
                        // [safety] (mut ref to static, by access to `USB_BUFFER`, `USB`) inside main + critical section
                        // [safety] (MaybeUninit::assume_init_mut, by access to `USB`) already initialized
                        unsafe { println_i!("i2c debug read done (result: {:?}) # {}", read_result, now_ms) };
                    });

                    let bytes = {
                        let i2c_ptr = I2C.get();
                        // [safety] (mut ref to static) inside main + only interrupts related register of `I2C` are used in interrupts and here we are not using these interrupt related registers, so no data races can happen because no data is really shared
                        // [safety] (MaybeUninit::assume_init_mut) `I2C` is initialized
                        let i2c = unsafe { (&mut *i2c_ptr).assume_init_mut() };

                        let byte1 = i2c.data().read().fifo_rdata().bits();
                        let byte2 = i2c.data().read().fifo_rdata().bits();
                        let byte3 = i2c.data().read().fifo_rdata().bits();

                        [byte1, byte2, byte3]
                    };

                    critical_section::with(|_cs| {
                        // [safety] (mut ref to static, by access to `USB_BUFFER`, `USB`) inside main + critical section
                        // [safety] (MaybeUninit::assume_init_mut, by access to `USB`) already initialized
                        unsafe { println_i!("i2c debug read data: {:02x} {:02x} {:02x}", bytes[0], bytes[1], bytes[2]) };
                    });

                    i2c_debug_state = I2CDebugState::Done;
                }
            },
            I2CDebugState::Done => {},
        }

        if qq_alarm_any_pending() {
            let mut do_print_1 = false;
            let mut do_print_2 = false;

            critical_section::with(|_cs| {
                // [safety] (mut ref to static, by access to `QQ_ALARM_QUEUE`) inside main + critical section (`pending_iter` dropped inside critical section)
                let pending_iter = unsafe { qq_alarm_consume_pending() };

                pending_iter.for_each(|qq_alarm_id| {
                    if qq_alarm_id == debug_print_id_1 {
                        do_print_1 = true;
                    } else if qq_alarm_id == debug_print_id_2 {
                        do_print_2 = true;
                    } else {
                        panic!("ajajaj id nesedi more");
                    }
                });
            });

            if do_print_1 {
                critical_section::with(|_cs| {
                    // [safety] (mut ref to static, by access to `USB_BUFFER`, `USB`) inside main + critical section
                    // [safety] (MaybeUninit::assume_init_mut, by access to `USB`) already initialized
                    unsafe { println_i!("debug print 1") };
                });

                // let next_print = SystemTimer::now() + SystemTimer::TICKS_PER_SECOND;

                // debug_print_id_1 = critical_section::with(|_cs| {
                //     // [safety] (mut ref to static, by access to `USB_BUFFER`, `USB`) inside main + critical section
                //     // [safety] (MaybeUninit::assume_init_mut, by access to `USB`) already initialized
                //     unsafe { println_i!("debug print 1") };

                //     // [safety] (mut ref to static, by access to `QQ_ALARM_QUEUE`, `ALARM0`) inside main + critical section
                //     // [safety] (MaybeUninit::assume_init_mut, by access to `ALARM0`) already initialized
                //     unsafe { qq_alarm_add(next_print) }
                // }).unwrap();
            }

            if do_print_2 {
                critical_section::with(|_cs| {
                    // [safety] (mut ref to static, by access to `USB_BUFFER`, `USB`) inside main + critical section
                    // [safety] (MaybeUninit::assume_init_mut, by access to `USB`) already initialized
                    unsafe { println_i!("debug print 2") };

                    // [safety] (mut ref to static, by access to `QQ_ALARM_QUEUE`, `ALARM0`) inside main + critical section
                    // [safety] (MaybeUninit::assume_init_mut, by access to `ALARM0`) already initialized
                    unsafe { qq_alarm_remove(debug_print_id_1) }.unwrap();
                });

                // let next_print = SystemTimer::now() + SystemTimer::TICKS_PER_SECOND * 3 / 2;

                // debug_print_id_2 = critical_section::with(|_cs| {
                //     // [safety] (mut ref to static, by access to `USB_BUFFER`, `USB`) inside main + critical section
                //     // [safety] (MaybeUninit::assume_init_mut, by access to `USB`) already initialized
                //     unsafe { println_i!("debug print 2") };

                //     // [safety] (mut ref to static, by access to `QQ_ALARM_QUEUE`, `ALARM0`) inside main + critical section
                //     // [safety] (MaybeUninit::assume_init_mut, by access to `ALARM0`) already initialized
                //     unsafe { qq_alarm_add(next_print) }
                // }).unwrap();
            }

            if !do_print_1 && !do_print_2 {
                critical_section::with(|_cs| {
                    // [safety] (mut ref to static, by access to `USB_BUFFER`, `USB`) inside main + critical section
                    // [safety] (MaybeUninit::assume_init_mut, by access to `USB`) already initialized
                    unsafe { println_i!("volaco podivne sa deje") };
                });
            }
        }
    }
}



// interrupt is small
#[interrupt]
fn I2C_EXT0() {
    let i2c_ptr = I2C.get();
    // [safety] (mut ref to static) inside interrupt + nested interrupts not enabled + interrupts related registers for `I2C` are not used in `main` after interrupts are enabled
    // [safety] (MaybeUninit::assume_init_mut) `I2C` is initialized inside main before interrupts are enabled
    let i2c = unsafe { (&mut *i2c_ptr).assume_init_mut() };

    let int_status = i2c.int_status().read().bits();

    // logically this should be done, but this is no-op because we know which interrupts are enables so we know that only valid bits can be set
    // let i2c_pending_interupts = I2CInterruptsStatus::from_bits(int_status).unwrap().bits();

    I2C_PENDING_INTERUPTS.fetch_or(int_status, Ordering::Relaxed);

    i2c.int_clr().write(|w| {
        w.trans_complete_int_clr().set_bit()
         .nack_int_clr().set_bit()
         .arbitration_lost_int_clr().set_bit()
         .time_out_int_clr().set_bit()
         .scl_main_st_to_int_clr().set_bit()
         .scl_st_to_int_clr().set_bit()
    });
}

// interrupt is small
//  - toggle led (only for debug pruposes)
//  - copy from usb buffer to usb fifo + flush <- this is main thing we want to do in interrupt
//  - optionally disable interrupt
#[interrupt]
fn USB_DEVICE() {
    let usb_ptr = USB.get();
    // [safety] (mut ref to static) inside interrupt + nested interrupts not enabled + all access outside interrupt guarded by critical section
    // [safety] (MaybeUninit::assume_init_mut) `USB` is initialized inside main before interrupts are enabled
    let usb = unsafe { (&mut *usb_ptr).assume_init_mut() };

    usb.int_clr().write(|w| w.serial_in_empty_int_clr().set_bit());

    USB_TIMEOUT.store(false, Ordering::Relaxed);
    
    let debug_led_ptr = DEBUG_LED.get();
    // [safety] (mut ref to static) inside interrupt + nested interrupts not enabled + not used in main after interrupts are enabled
    // [safety] (MaybeUninit::assume_init_mut) `DEBUG_LED` is initialized inside main before interrupts are enabled
    let debug_led = unsafe { (&mut *debug_led_ptr).assume_init_mut() };

    debug_led.set_high().unwrap();

    let usb_buffer_ptr = USB_BUFFER.get();
    // [safety] (mut ref to static) inside interrupt + nested interrupts not enabled + all access outside interrupt guarded by critical section
    let usb_buffer = unsafe { &mut *usb_buffer_ptr };

    while usb.ep1_conf().read().serial_in_ep_data_free().bit_is_set() {
        match usb_buffer.pop() {
            Some(byte) => {
                usb.ep1().write(|w| w.rdwr_byte().variant(byte));
            },
            None => {
                usb.ep1_conf().write(|w| w.wr_done().set_bit()); // flush
                break;
            }
        }
    }

    if usb_buffer.len() == 0 {
        usb.int_ena().modify(|_, w| w.serial_in_empty_int_ena().clear_bit());
        usb.int_clr().write(|w| w.serial_in_empty_int_clr().set_bit());

        // alarm0.enable_interrupt(false);
    } else {
        // alarm0.set_target(SystemTimer::now() + USB_TIMEOUT_THRESHOLD);
    }

    // alarm0.clear_interrupt();
}

// interrupt is small
// #[interrupt]
// fn SYSTIMER_TARGET0() {
//     let alarm0_ptr = ALARM0.get();
//     // [safety] (mut ref to static) inside interrupt + nested interrupts not enabled + all access outside interrupt guarded by critical section
//     // [safety] (MaybeUninit::assume_init_mut) `ALARM0` is initialized inside main before interrupts are enabled
//     let alarm0 = unsafe { (&mut *alarm0_ptr).assume_init_mut() };

//     alarm0.clear_interrupt();

//     USB_TIMEOUT.store(true, Ordering::Relaxed);

//     let debug_led_ptr = DEBUG_LED.get();
//     // [safety] (mut ref to static) inside interrupt + nested interrupts not enabled + not used in main after interrupts are enabled
//     // [safety] (MaybeUninit::assume_init_mut) `DEBUG_LED` is initialized inside main before interrupts are enabled
//     let debug_led = unsafe { (&mut *debug_led_ptr).assume_init_mut() };

//     debug_led.set_low().unwrap();
// }

// interrupt is small
#[interrupt]
fn SYSTIMER_TARGET0() {
    let alarm_ptr = ALARM0.get();
    // [safety] (mut ref to static) inside interrupt + nested interrupts not enabled + all access outside interrupt guarded by critical section
    // [safety] (MaybeUninit::assume_init_mut) `ALARM0` is initialized inside main before interrupts are enabled
    let alarm = unsafe { (&mut *alarm_ptr).assume_init_mut() };

    alarm.clear_interrupt();

    let alarm_queue_ptr = QQ_ALARM_QUEUE.get();
    // [safety] (mut ref to static) inside interrupt + nested interrupts not enabled + all access outside interrupt guarded by critical section
    let alarm_queue = unsafe { &mut *alarm_queue_ptr };

    let now = SystemTimer::now();

    let mut min_wake_at = None;
    let mut any_pending = false;

    for qq_alarm in alarm_queue.queue.iter_mut().filter_map(|qq_alarm| qq_alarm.as_mut()).filter(|qq_alarm| qq_alarm.state == QQAlarmState::Waiting) {
        let wake_at = qq_alarm.m.wake_at;

        if wake_at <= now {
            any_pending = true;
            qq_alarm.state = QQAlarmState::Pending;
        } else {
            min_wake_at = Some(min_wake_at.map_or(wake_at, |min_wake_at| cmp::min(min_wake_at, wake_at)));
        }
    }

    if any_pending {
        QQ_ALARM_ANY_PENDING.store(true, Ordering::Release);
    }

    alarm_queue.next_wakeup = min_wake_at;

    if let Some(min_wake_at) = min_wake_at {
        alarm.set_target(min_wake_at);
    } else {
        alarm.enable_interrupt(false);
    }
}