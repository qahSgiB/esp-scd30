use core::iter;

use esp_hal::{gpio::{Input, InputPin, Pull}, peripheral::{Peripheral, PeripheralRef}, peripherals::{self, RMT, SYSTEM}, rmt::PulseCode};

use crate::interrupts::RMTInterruptStatus;



#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RMTError {
    Unknown(RMTInterruptStatus),
}

impl RMTError {
    pub fn from_interrupt_flags(interrupt: RMTInterruptStatus) -> Option<RMTError> {
        interrupt.is_error().then_some(RMTError::Unknown(interrupt))
    }
}


pub struct RmtClockConfig {
    pub selection: u8,
    pub div_num: u8,
    pub div_a: u8,
    pub div_b: u8,
}

pub fn config_clock(system: PeripheralRef<SYSTEM>, config: RmtClockConfig) {
    // TODO: safety
    system.rmt_sclk_conf().modify(|_, w| unsafe {
        w
            .sclk_sel().bits(config.selection)
            .sclk_div_num().bits(config.div_num)
            .sclk_div_a().bits(config.div_a)
            .sclk_div_b().bits(config.div_b)
    });
}

pub fn config(rmt: PeripheralRef<RMT>, use_fifo: bool) {
    rmt.sys_conf().modify(|_, w| w.apb_fifo_mask().bit(!use_fifo)); // fifo on/off
}

pub struct RmtRxChConfig {
    pub clock_div: u8,
    pub idle_thresh: u16,
}

pub fn ch2_config(rmt: PeripheralRef<RMT>, config: RmtRxChConfig) {
    rmt.ch2_rx_conf0().modify(|_, w| unsafe {
        w
            .div_cnt().bits(config.clock_div)
            .idle_thres().bits(config.idle_thresh)
            .carrier_en().bit(false) // disable demodulation
    });

    rmt.ch2_rx_conf1().modify(|_, w| w.conf_update().set_bit()); // sync
}

pub fn ch2_enable_interrupts(rmt: PeripheralRef<RMT>) {
    rmt.int_ena().modify(|_, w| {
        w
            .ch2_rx_end().bit(true)
            .ch2_rx_err().bit(true)
    });
}

fn ch2_rx_enable(rmt: PeripheralRef<RMT>, enable: bool) {
    rmt.ch2_rx_conf1().modify(|_, w| w.rx_en().bit(enable)); // enable recieving
    rmt.ch2_rx_conf1().modify(|_, w| w.conf_update().set_bit()); // sync
}

pub fn ch2_start(rmt: PeripheralRef<RMT>) {
    ch2_rx_enable(rmt, true);
}


pub fn setup_pins<'a, PIN>(
    pin: impl Peripheral<P = PIN> + 'a,
) -> Input<'a, PIN>
where
    PIN: InputPin
{
    let pin = Input::new(pin, Pull::None);

    let pin_num = 10;

    // TODO
    // SAFETY: only pin owned by this function is accessed ???
    let pac_gpio = unsafe { peripherals::GPIO::steal() };
    let pac_io_mux = unsafe { peripherals::IO_MUX::steal() };

    // TODO: safety
    pac_io_mux.gpio(pin_num).modify(|_, w| unsafe {
        w.mcu_sel().bits(1) // set alternate function to 1 - use gpio matrix
    });
    pac_gpio.func_in_sel_cfg(71).modify(|_, w| unsafe {
        w
            .sel().set_bit() // use gpio matrix for input
            .in_sel().bits(pin_num as u8) // connect input to gpio via gpio matrix
    });

    pin
}


// TODO: name
pub struct HalfPulseCode {
    pub level: bool,
    pub length: u16,
}

impl HalfPulseCode {
    pub fn from_pulse_code(pulse_code: PulseCode) -> (HalfPulseCode, HalfPulseCode) {
        (
            HalfPulseCode { level: pulse_code.level1, length: pulse_code.length1 },
            HalfPulseCode { level: pulse_code.level2, length: pulse_code.length2 },
        )
    }
}


pub fn ch2_fifo_iter<'a>(mut rmt: PeripheralRef<'a, RMT>, pause_rx: bool) -> impl Iterator<Item = HalfPulseCode> + 'a {
    if pause_rx {
        ch2_rx_enable(rmt.reborrow(), false);
    }

    let mut end_marker = false;

    iter::repeat_with(move || {
        if end_marker {
            return [None, None];
        }
            
        let (pulse1, pulse2) = HalfPulseCode::from_pulse_code(PulseCode::from(rmt.ch2data().read().bits()));

        let pulse1_zero = pulse1.length == 0;
        let pulse2_zero = pulse2.length == 0;

        if pulse1_zero || pulse2_zero {
            end_marker = true;
        }
        
        [(!pulse1_zero).then_some(pulse1), (!pulse2_zero).then_some(pulse2)]
    })
        .flatten()
        .take_while(Option::is_some)
        .filter_map(|code| code)
}

pub fn ch2_reset_after_recieving<'a>(rmt: PeripheralRef<'a, RMT>, rx_paused: bool) {
    rmt.ch2_rx_conf1().modify(|_, w| {
        w
            .mem_wr_rst().bit(true) // reset RX channel's RAM write address
            .apb_mem_rst().bit(true) // reset fifo
            .mem_owner().bit(true) // set owner back to peripheral ???
    });

    if rx_paused {
        ch2_rx_enable(rmt, true);
    }
}