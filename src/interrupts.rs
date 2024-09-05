use core::sync::atomic::{AtomicU32, Ordering};

use bitflags::bitflags;
use esp_hal::{interrupt::{self, Priority}, macros::handler, peripherals::{Interrupt, GPIO, I2C0, RMT, SYSTIMER, USB_DEVICE}};



bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct USBInterruptStatus: u32 {
        const SERIAL_IN_EMPTY = 1 << 3;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SystimerTartet0InterruptStatus: u32 {
        const TARGET = 1 << 0;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct I2CInterruptStatus: u32 {
        const ARBITRATION_LOST = 1 << 5;
        const TRANSACTION_COMPLETE = 1 << 7;
        const TIME_OUT = 1 << 8;
        const NACK = 1 << 10;
        const SCL_ST_TIME_OUT = 1 << 13;
        const SCL_MAIN_ST_TIME_OUT = 1 << 14;
    }
}

impl I2CInterruptStatus {
    pub fn is_error(&self) -> bool {
        self.intersects(
            I2CInterruptStatus::ARBITRATION_LOST
            | I2CInterruptStatus::TIME_OUT
            | I2CInterruptStatus::NACK
            | I2CInterruptStatus::SCL_ST_TIME_OUT
            | I2CInterruptStatus::SCL_MAIN_ST_TIME_OUT
        )
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct GPIOInterruptStatus: u32 {
        const GPIO6 = 1 << 6;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct RMTInterruptStatus: u32 {
        const CH2_END = 1 << 2;
        const CH2_ERROR = 1 << 6;
    }
}

impl RMTInterruptStatus {
    pub fn is_error(&self) -> bool {
        self.intersects(RMTInterruptStatus::CH2_ERROR)
    }
}



pub fn usb_interrupt_enable(priority: Option<Priority>) {
    // [todo] safety
    unsafe { interrupt::bind_interrupt(Interrupt::USB_DEVICE, usb_handler.handler()) };
    interrupt::enable(Interrupt::USB_DEVICE, priority.unwrap_or(usb_handler.priority())).unwrap();
}

pub fn usb_interrupt_get() -> USBInterruptStatus {
    USBInterruptStatus::from_bits_truncate(USB_PENDING_INTERRUPTS.load(Ordering::Relaxed))
}

pub fn usb_interrupt_clear(interrupts: USBInterruptStatus) {
    USB_PENDING_INTERRUPTS.fetch_and((!interrupts).bits(), Ordering::Relaxed);
}

pub fn usb_interrupt_get_and_clear(interrupts: USBInterruptStatus) -> USBInterruptStatus {
    USBInterruptStatus::from_bits_truncate(USB_PENDING_INTERRUPTS.fetch_and((!interrupts).bits(), Ordering::Relaxed)).intersection(interrupts)
}


static USB_PENDING_INTERRUPTS: AtomicU32 = AtomicU32::new(USBInterruptStatus::empty().bits());


#[handler]
fn usb_handler() {
    // [todo] safety
    let usb = unsafe { USB_DEVICE::steal() };

    USB_PENDING_INTERRUPTS.fetch_or(usb.int_st().read().bits(), Ordering::Relaxed);

    // SAFETY: clear all interrupts, bits are valid according to specification
    usb.int_clr().write(|w| unsafe { w.bits(0xffff) });
}



pub fn systimer_target0_interrupt_enable(priority: Option<Priority>) {
    // [todo] safety
    unsafe { interrupt::bind_interrupt(Interrupt::SYSTIMER_TARGET0, systimer_target0_handler.handler()) };
    interrupt::enable(Interrupt::SYSTIMER_TARGET0, priority.unwrap_or(systimer_target0_handler.priority())).unwrap();
}

pub fn systimer_target0_interrupt_get() -> SystimerTartet0InterruptStatus {
    SystimerTartet0InterruptStatus::from_bits_truncate(SYSTIMER_TARGET0_PENDING_INTERRUPTS.load(Ordering::Relaxed))
}

pub fn systimer_target0_interrupt_clear(interrupts: SystimerTartet0InterruptStatus) {
    SYSTIMER_TARGET0_PENDING_INTERRUPTS.fetch_and((!interrupts).bits(), Ordering::Relaxed);
}

pub fn systimer_target0_interrupt_get_and_clear(interrupts: SystimerTartet0InterruptStatus) -> SystimerTartet0InterruptStatus {
    SystimerTartet0InterruptStatus::from_bits_truncate(SYSTIMER_TARGET0_PENDING_INTERRUPTS.fetch_and((!interrupts).bits(), Ordering::Relaxed)).intersection(interrupts)
}


static SYSTIMER_TARGET0_PENDING_INTERRUPTS: AtomicU32 = AtomicU32::new(SystimerTartet0InterruptStatus::empty().bits());


#[handler(priority = esp_hal::interrupt::Priority::Priority10)]
fn systimer_target0_handler() {
    // [todo]
    let systimer = unsafe { SYSTIMER::steal() };

    SYSTIMER_TARGET0_PENDING_INTERRUPTS.fetch_or(systimer.int_st().read().bits() & 0b1, Ordering::Relaxed);

    // SAFETY: clear all interrupts, bits are valid according to specification
    systimer.int_clr().write(|w| unsafe { w.bits(0b1) });
}



pub fn i2c_interrupt_enable(priority: Option<Priority>) {
    // [todo] safety
    unsafe { interrupt::bind_interrupt(Interrupt::I2C_EXT0, i2c_handler.handler()) };
    interrupt::enable(Interrupt::I2C_EXT0, priority.unwrap_or(i2c_handler.priority())).unwrap();
}

pub fn i2c_interrupt_get() -> I2CInterruptStatus {
    I2CInterruptStatus::from_bits_truncate(I2C_PENDING_INTERRUPTS.load(Ordering::Relaxed))
}

pub fn i2c_interrupt_clear(interrupts: I2CInterruptStatus) {
    I2C_PENDING_INTERRUPTS.fetch_and((!interrupts).bits(), Ordering::Relaxed);
}

pub fn i2c_interrupt_get_and_clear(interrupts: I2CInterruptStatus) -> I2CInterruptStatus {
    I2CInterruptStatus::from_bits_truncate(I2C_PENDING_INTERRUPTS.fetch_and((!interrupts).bits(), Ordering::Relaxed)).intersection(interrupts)
}


static I2C_PENDING_INTERRUPTS: AtomicU32 = AtomicU32::new(I2CInterruptStatus::empty().bits());


#[handler]
fn i2c_handler() {
    // [todo]
    let i2c = unsafe { I2C0::steal() };

    I2C_PENDING_INTERRUPTS.fetch_or(i2c.int_st().read().bits(), Ordering::Relaxed);

    // SAFETY: clear all interrupts, bits are valid according to specification
    i2c.int_clr().write(|w| unsafe { w.bits(0b0111_1111_1111_1111_1111) });
}



pub fn gpio_interrupt_enable(priority: Option<Priority>) {
    // [todo] safety
    unsafe { interrupt::bind_interrupt(Interrupt::GPIO, gpio_handler.handler()) };
    interrupt::enable(Interrupt::GPIO, priority.unwrap_or(gpio_handler.priority())).unwrap();
}

pub fn gpio_interrupt_get() -> GPIOInterruptStatus {
    GPIOInterruptStatus::from_bits_truncate(GPIO_PENDING_INTERRUPTS.load(Ordering::Relaxed))
}

pub fn gpio_interrupt_clear(interrupts: GPIOInterruptStatus) {
    GPIO_PENDING_INTERRUPTS.fetch_and((!interrupts).bits(), Ordering::Relaxed);
}

pub fn gpio_interrupt_get_and_clear(interrupts: GPIOInterruptStatus) -> GPIOInterruptStatus {
    GPIOInterruptStatus::from_bits_truncate(GPIO_PENDING_INTERRUPTS.fetch_and((!interrupts).bits(), Ordering::Relaxed)).intersection(interrupts)
}


static GPIO_PENDING_INTERRUPTS: AtomicU32 = AtomicU32::new(GPIOInterruptStatus::empty().bits());


#[handler]
fn gpio_handler() {
    // TODO
    let gpio = unsafe { GPIO::steal() };

    GPIO_PENDING_INTERRUPTS.fetch_or(gpio.status().read().bits(), Ordering::Relaxed);

    // SAFETY: clear all interrupts, bits are valid according to specification
    gpio.status_w1tc().write(|w| unsafe { w.bits(0b0111_1111_1111_1111_1111) });
}



pub fn rmt_interrupt_enable(priority: Option<Priority>) {
    // [todo] safety
    unsafe { interrupt::bind_interrupt(Interrupt::RMT, rmt_handler.handler()) };
    interrupt::enable(Interrupt::RMT, priority.unwrap_or(rmt_handler.priority())).unwrap();
}

pub fn rmt_interrupt_get() -> RMTInterruptStatus {
    RMTInterruptStatus::from_bits_truncate(RMT_PENDING_INTERRUPTS.load(Ordering::Relaxed))
}

pub fn rmt_interrupt_clear(interrupts: RMTInterruptStatus) {
    RMT_PENDING_INTERRUPTS.fetch_and((!interrupts).bits(), Ordering::Relaxed);
}

pub fn rmt_interrupt_get_and_clear(interrupts: RMTInterruptStatus) -> RMTInterruptStatus {
    RMTInterruptStatus::from_bits_truncate(RMT_PENDING_INTERRUPTS.fetch_and((!interrupts).bits(), Ordering::Relaxed)).intersection(interrupts)
}


static RMT_PENDING_INTERRUPTS: AtomicU32 = AtomicU32::new(RMTInterruptStatus::empty().bits());


#[handler]
fn rmt_handler() {
    // TODO
    let rmt = unsafe { RMT::steal() };

    RMT_PENDING_INTERRUPTS.fetch_or(rmt.int_st().read().bits(), Ordering::Relaxed);

    // SAFETY: clear all interrupts, bits are valid according to specification
    rmt.int_clr().write(|w| unsafe { w.bits(0b0011_1111_1111_1111) });
}