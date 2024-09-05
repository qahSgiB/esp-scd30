/* helper functions for using RMT peripheral reciever channels */



use core::borrow::Borrow;

use esp_hal::{
    gpio::{OutputPin, OutputSignal},
    peripheral::PeripheralRef,
    peripherals::{RMT, SYSTEM}, rmt::PulseCode
};



pub struct RmtClockConfig {
    pub selection: u8,
    pub div_num: u8,
    pub div_a: u8,
    pub div_b: u8,
}

pub fn rmt_clock_config<'a>(system: PeripheralRef<'a, SYSTEM>, config: RmtClockConfig) {
    system.rmt_sclk_conf().modify(|_, w| {
        w.sclk_sel().variant(config.selection)
         .sclk_div_num().variant(config.div_num)
         .sclk_div_a().variant(config.div_a)
         .sclk_div_b().variant(config.div_b)
    });

    system.rmt_conf().modify(|_, w| {
        w.rmt_clk_en().set_bit()
    });
}

pub fn rmt_config<'a>(rmt: PeripheralRef<'a, RMT>, ram_direct: bool) {
    rmt.sys_conf().modify(|_, w| {
        w.apb_fifo_mask().bit(ram_direct)
    });
}

#[derive(Debug, Clone, Copy)]
pub enum RmtChannelIdleConfig {
    EndMarker,
    Level(bool),
}

impl RmtChannelIdleConfig {
    fn into_regs(&self) -> (bool, bool) {
        match *self {
            RmtChannelIdleConfig::EndMarker => (false, false),
            RmtChannelIdleConfig::Level(level) => (true, level),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum RmtChannelCarrierConfig {
    Disabled,
    Enabled {
        on_level: bool,
        on_idle: bool,
        duty_low: u16,
        duty_high: u16,
    }
}

impl RmtChannelCarrierConfig {
    fn into_regs(&self) -> (bool, bool, bool, u16, u16) {
        match *self {
            RmtChannelCarrierConfig::Disabled => (false, false, false, 0, 0),
            RmtChannelCarrierConfig::Enabled { on_level, on_idle, duty_low, duty_high } => (true, on_level, on_idle, duty_low, duty_high),
        }
    }
}

pub struct RmtChannelConfig {
    pub div: u8,
    pub carrier: RmtChannelCarrierConfig,
    pub idle: RmtChannelIdleConfig,
}

pub fn rmt_ch0_config<'a>(rmt: PeripheralRef<'a, RMT>, pin: &mut impl OutputPin, config: RmtChannelConfig) {
    pin.connect_peripheral_to_output(OutputSignal::RMT_SIG_0);

    let (idle_out_en, idle_out_lv) = config.idle.into_regs();
    let (carrier_en, carrier_out_lv, carrier_eff_en, carrier_high, carrier_low) = config.carrier.into_regs();

    rmt.ch0_tx_conf0().modify(|_, w| {
        w.div_cnt().variant(config.div)
         .carrier_en().bit(carrier_en)
         .carrier_out_lv().bit(carrier_out_lv)
         .carrier_eff_en().bit(carrier_eff_en)
         .idle_out_en().bit(idle_out_en)
         .idle_out_lv().bit(idle_out_lv)
    });

    if carrier_en {
        rmt.ch0carrier_duty().write(|w| {
            w.carrier_high().variant(carrier_high)
             .carrier_low().variant(carrier_low)
        });
    }

    rmt.ch0_tx_conf0().modify(|_, w| {
        w.conf_update().set_bit()
    });
}

/// # safety:
/// This function assumes that iterator yields at most 48 pulse codes, otherwise it causes undefined behavior.
/// (Ram block for one channel has space for maximum of 48 pulse code blocks.)
///
pub unsafe fn rmt_ch0_fill_ram_assume_len(pulse_codes: impl Iterator<Item = impl Borrow<PulseCode>>) {
    let rmt_ram_ptr = 0x60006400 as *mut u32;

    for (rmt_pulse_index, rmt_pulse) in pulse_codes.enumerate() {
        // [todo] safety details
        unsafe { rmt_ram_ptr.add(rmt_pulse_index).write_volatile((*rmt_pulse.borrow()).into()) };
    }
}

pub fn rmt_ch0_interupts_clear_all<'a>(rmt: PeripheralRef<'a, RMT>) {
    rmt.int_clr().write(|w| {
        w.ch0_tx_end().set_bit()
         .ch0_tx_err().set_bit()
         .ch0_tx_thr_event().set_bit()
         .ch0_tx_loop().set_bit()
    });
}

pub fn rmt_ch0_start<'a>(rmt: PeripheralRef<'a, RMT>) {
    rmt.ref_cnt_rst().write(|w| {
        w.tx_ref_cnt_rst().set_bit()
    });

    rmt.ch0_tx_conf0().modify(|_, w| {
        w.tx_start().set_bit()
         .mem_rd_rst().set_bit()
    });
}

pub fn rmt_ch0_is_done<'a>(rmt: PeripheralRef<'a, RMT>) -> Result<bool, ()> {
    if rmt.int_raw().read().ch0_tx_err().bit() {
        rmt.int_clr().write(|w| w.ch0_tx_err().set_bit());
        Err(())
    } else if rmt.int_raw().read().ch0_tx_end().bit() {
        rmt.int_clr().write(|w| w.ch0_tx_end().set_bit());
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn rmt_ch0_wait_done<'a>(mut rmt: PeripheralRef<'a, RMT>) -> Result<(), ()> {
    loop {
        let rmt_ch0_status = rmt_ch0_is_done(rmt.reborrow());
        match rmt_ch0_status {
            Ok(true) => { return Ok(()) },
            Err(()) => { return Err(()) },
            _ => {}
        }
    }
}