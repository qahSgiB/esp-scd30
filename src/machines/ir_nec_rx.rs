use core::fmt::Write;

use esp_hal::{gpio::{Input, InputPin}, interrupt::Priority, peripheral::{Peripheral, PeripheralRef}, peripherals::{RMT, SYSTEM}};

use crate::{interrupts::{self, RMTInterruptStatus}, pac_utils::rmt::{self as rmt_utils, RMTError, RmtClockConfig, RmtRxChConfig}};



fn in_range(value: u16, min: u16, max: u16) -> bool {
    min <= value && value <= max
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NecDecodeError {
    InvalidPulseCountTooShort,
    InvalidPulseCountTooLong,
    Start1InvalidLength,
    Start0InvalidLength,
    Data1InvalidLength(u16),
    Data0InvalidLength,
    Last1InvalidLength,
    AddressInvertedNotMatching,
    MessageInvertedNotMatching,
}

#[derive(Debug, Clone, Copy)]
enum NecMessage {
    Message {
        address: u8,
        message: u8,
    },
    Repeat,
}

#[derive(Debug, Clone, Copy)]
struct NecIrTimingConfig {
    short: u16, // duration of shortest nec pulse (560 us),
    tol_div: u16,
    tol_num: u16,
}

struct NecDecoder {
    short_min: u16,
    short_max: u16,
    long_min: u16,
    long_max: u16,
    start_1_min: u16,
    start_1_max: u16,
    start_0_min: u16,
    start_0_max: u16,
    repeat_min: u16,
    repeat_max: u16,
}

impl NecDecoder {
    const LONG_MUL: u16 = 3;
    const START_1_MUL: u16 = 16;
    const START_0_MUL: u16 = 8;
    const REPEAT_MUL: u16 = 4;

    const MS_1: u8 = 0b1000_0000;


    fn new(config: NecIrTimingConfig) -> Self {
        Self {
            short_min:   config.short *                     (config.tol_div - config.tol_num) / config.tol_div,
            short_max:   config.short *                     (config.tol_div + config.tol_num) / config.tol_div,
            long_min:    config.short * Self::LONG_MUL    * (config.tol_div - config.tol_num) / config.tol_div,
            long_max:    config.short * Self::LONG_MUL    * (config.tol_div + config.tol_num) / config.tol_div,
            start_1_min: config.short * Self::START_1_MUL * (config.tol_div - config.tol_num) / config.tol_div,
            start_1_max: config.short * Self::START_1_MUL * (config.tol_div + config.tol_num) / config.tol_div,
            start_0_min: config.short * Self::START_0_MUL * (config.tol_div - config.tol_num) / config.tol_div,
            start_0_max: config.short * Self::START_0_MUL * (config.tol_div + config.tol_num) / config.tol_div,
            repeat_min:  config.short * Self::REPEAT_MUL  * (config.tol_div - config.tol_num) / config.tol_div,
            repeat_max:  config.short * Self::REPEAT_MUL  * (config.tol_div + config.tol_num) / config.tol_div,
        }
    }

    fn decode_u8(&self, pulses: impl Iterator<Item = u16>) -> Result<u8, NecDecodeError> {
        let (n, counter) = pulses.take(16).array_chunks::<2>().try_fold((0u8, 0usize), |(n, counter), [pulse1, pulse0]| {
            if !in_range(pulse1, self.short_min, self.short_max) {
                return Err(NecDecodeError::Data1InvalidLength(pulse1));
            }

            if in_range(pulse0, self.short_min, self.short_max) {
                Ok((n >> 1, counter + 1))
            } else if in_range(pulse0, self.long_min, self.long_max) {
                Ok(((n >> 1) | Self::MS_1, counter + 1))
            } else {
                Err(NecDecodeError::Data0InvalidLength)
            }
        })?;

        if counter != 8 {
            Err(NecDecodeError::InvalidPulseCountTooShort)
        } else {
            Ok(n)
        }
    }

    fn decode(&self, mut pulses: impl Iterator<Item = u16>) -> Result<NecMessage, NecDecodeError> {
        let start1 = pulses.next().ok_or(NecDecodeError::InvalidPulseCountTooShort)?;

        if !in_range(start1, self.start_1_min, self.start_1_max) {
            return Err(NecDecodeError::Start1InvalidLength);
        }

        let start0 = pulses.next().ok_or(NecDecodeError::InvalidPulseCountTooShort)?;

        if in_range(start0, self.repeat_min, self.repeat_max) {
            return Ok(NecMessage::Repeat);
        } else if !in_range(start0, self.start_0_min, self.start_0_max) {
            return Err(NecDecodeError::Start0InvalidLength);
        }

        let address = self.decode_u8(pulses.by_ref())?;
        let address_inverted = self.decode_u8(pulses.by_ref())?;

        if address ^ address_inverted != 0b1111_1111 {
            return Err(NecDecodeError::AddressInvertedNotMatching);
        }

        let message = self.decode_u8(pulses.by_ref())?;
        let message_inverted = self.decode_u8(pulses.by_ref())?;

        if message ^ message_inverted != 0b1111_1111 {
            return Err(NecDecodeError::MessageInvertedNotMatching);
        }

        let last = pulses.next().ok_or(NecDecodeError::InvalidPulseCountTooShort)?;

        if !in_range(last, self.short_min, self.short_max) {
            return Err(NecDecodeError::Last1InvalidLength);
        }

        if pulses.next() != None {
            return Err(NecDecodeError::InvalidPulseCountTooLong);
        }

        Ok(NecMessage::Message {
            address,
            message,
        })
    }
}



enum IrNecRxState {
    Active,
    Error,
}

pub struct IrNecRx<'a, 'b, PIN> {
    rmt: PeripheralRef<'a, RMT>,
    pin: Input<'b, PIN>, // TODO: same as with `SdcSimpleMeassurment`
    nec_decoder: NecDecoder,
    state: IrNecRxState,
}

impl<'a, 'b, PIN> IrNecRx<'a, 'b, PIN>
where
    PIN: InputPin
{
    pub fn new<'c>(
        rmt: impl Peripheral<P = RMT> + 'a,
        pin: impl Peripheral<P = PIN> + 'b,
        system: impl Peripheral<P = SYSTEM> + 'c
    ) -> Self {
        let mut rmt = rmt.into_ref();

        rmt_utils::config_clock(system.into_ref(), RmtClockConfig {
            selection: 1, // using PPL_F80M_CLK (80 MHz)
            div_num: 224 - 1, // rmt_sclk F = 25 / 7 e5 Hz = 2500 / 7 KHz (T = 2.8 us)
            div_a: 0,
            div_b: 0,
        });

        rmt_utils::config(rmt.reborrow(), true);

        // TODO: maybe test idle_tresh
        rmt_utils::ch2_config(rmt.reborrow(), RmtRxChConfig {
            clock_div: 10, // clk_div T = 28 us (=> small pulse = 20 ticks)
            idle_thresh: 714, // 19.992 ms (~ 20 ms)
        });

        rmt_utils::ch2_enable_interrupts(rmt.reborrow());

        let pin = rmt_utils::setup_pins(pin);

        // TODO: lower tolerance maybe, when ir sensor electric connection is better
        let nec_decoder = NecDecoder::new(NecIrTimingConfig {
            short: 20,
            tol_div: 2, // 50% tolerance
            tol_num: 1,
        });

        Self {
            rmt,
            pin,
            nec_decoder,
            state: IrNecRxState::Active,
        }
    }

    pub fn enable_interrupt(&mut self) {
        interrupts::rmt_interrupt_enable(Some(Priority::Priority5));
    }

    pub fn start(&mut self) {
        rmt_utils::ch2_start(self.rmt.reborrow());
    }

    pub fn update(&mut self, usb_writer: &mut impl Write) -> bool {
        match self.state {
            IrNecRxState::Active => {
                let pending_interrupts = interrupts::rmt_interrupt_get_and_clear(RMTInterruptStatus::CH2_END | RMTInterruptStatus::CH2_ERROR);

                if pending_interrupts.is_empty() {
                    return false;
                }

                if let Some(err) = RMTError::from_interrupt_flags(pending_interrupts) {
                    let _ = writeln!(usb_writer, "rmt rx error : {:?}", err);

                    self.state = IrNecRxState::Error;
                } else {
                    // interrupt is `CH2_END`

                    // we assume that level's are alternating and that pulse code sequance starts with level 1

                    let recieved = rmt_utils::ch2_fifo_iter(self.rmt.reborrow(), false).map(|pulse| pulse.length);

                    let nec_decode_result = self.nec_decoder.decode(recieved);
                    rmt_utils::ch2_reset_after_recieving(self.rmt.reborrow(), false);

                    match nec_decode_result {
                        Ok(NecMessage::Repeat) => {
                            let _ = writeln!(usb_writer, "rmt recieved : REPEAT");
                        },
                        Ok(NecMessage::Message { address, message }) => {
                            let _ = writeln!(usb_writer, "rmt recieved : ADDRESS {} MESSAGE {}", address, message);
                        },
                        Err(err) => {
                            let _ = writeln!(usb_writer, "rmt decoding error : {:?}", err);

                            // self.state = IrNecRxState::Error;
                        },
                    }
                }

                true
            },
            IrNecRxState::Error => false,
        }
    }
}