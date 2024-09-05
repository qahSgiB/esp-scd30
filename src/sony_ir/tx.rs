use core::{iter, ops::IndexMut};

use esp_hal::{gpio::OutputPin, peripheral::PeripheralRef, peripherals::{RMT, SYSTEM}, rmt::PulseCode, systimer::SystemTimer};


use crate::rmt_tx::{self, RmtChannelCarrierConfig, RmtChannelConfig, RmtChannelIdleConfig, RmtClockConfig};

use super::{SonyIRCommand, SonyIRRawCommand};



#[derive(Debug, Clone, Copy)]
pub enum SonyIRError {
    UnsendableCommand,
    RMTPeripheral,
    EncoderBufferFull,
}


pub fn sony_ir_clock_config<'a>(system: PeripheralRef<'a, SYSTEM>) {
    rmt_tx::rmt_clock_config(system, RmtClockConfig {
        selection: 1,
        div_num: 249,
        div_a: 0,
        div_b: 0,
    });
}

pub fn sony_ir_ch0_config<'a>(mut rmt: PeripheralRef<'a, RMT>, pin: &mut impl OutputPin) {
    rmt_tx::rmt_ch0_config(rmt.reborrow(), pin, RmtChannelConfig {
        div: 192,
        carrier: RmtChannelCarrierConfig::Enabled { on_level: true, on_idle: true, duty_low: 6, duty_high: 2 },
        idle: RmtChannelIdleConfig::Level(false),
    });
    rmt_tx::rmt_ch0_interupts_clear_all(rmt.reborrow());
}


/// Same as `SonyIRRawCommand` but ensures that command is sendable using one block of RMT RAM (`bits` < 48).
/// `SonyIRRawSendableCommand` can be created only using `from_raw` or `from_command`, both of these ensures this condition.
/// 
#[derive(Debug, Clone, Copy)]
struct SonyIRRawSendableCommand {
    data: u32,
    bits: u8,
}

impl SonyIRRawSendableCommand {
    fn from_raw(command: SonyIRRawCommand) -> Option<SonyIRRawSendableCommand> {
        if command.bits == 0 || command.bits >= 48 {
            None
        } else {
            Some(SonyIRRawSendableCommand { data: command.data, bits: command.bits })
        }
    }

    fn from_command(command: SonyIRCommand) -> Option<SonyIRRawSendableCommand> {
        SonyIRRawSendableCommand::from_raw(SonyIRRawCommand::from_command(command))
    }
}

fn sony_ir_ch0_fill_ram_raw(command: SonyIRRawSendableCommand) {
    let mut data = command.data;

    let pulse_codes_start = iter::once(PulseCode {
        level1: true,
        length1: 4,
        level2: false,
        length2: 1
    });

    let pulse_codes = iter::repeat_with(move || {
        let bit = (data & 0b1) as u8;
        data >>= 1;
        bit
    }).take((command.bits - 1) as usize).map(|bit| PulseCode {
        level1: true,
        length1: (bit + 1) as u16, /* same as: if bit == 1 { 2 } else { 1 } */
        level2: false,
        length2: 1
    });

    let pulse_codes_end = iter::once(PulseCode {
        level1: true,
        length1: (((data >> (command.bits - 1)) & 0b1) + 1) as u16,
        level2: false,
        length2: 0
    });

    /* safety: `command.bits` is less than 48 (ensured by `SonyIRRawSendableCommand`), which means that iterator chain length is less or eqaul to 48 */
    unsafe { rmt_tx::rmt_ch0_fill_ram_assume_len(pulse_codes_start.chain(pulse_codes).chain(pulse_codes_end)) };
}

// [todo] maybe better error
/// returns Err(SonyIRError::UnsendableCommand) when command doesn't fit into RMT RAM (total command bit count >= 48), can happen only when using `SonyIRCommand::Raw` variant */
pub fn sony_ir_ch0_fill_ram(command: SonyIRCommand) -> Result<(), SonyIRError> {
    let command = SonyIRRawSendableCommand::from_command(command).ok_or(SonyIRError::UnsendableCommand)?;
    sony_ir_ch0_fill_ram_raw(command);
    Ok(())
}



#[derive(Debug, Clone, Copy)]
pub enum SonyIREncoderPause {
    FromStart(u64),
    FromEnd(u64),
}

enum SonyIREncoderState {
    None,
    TxPauseFromEnd(u64),
    TxPauseFromStart(u64),
    Paused(u64),
}

pub struct SonyIREncoder<const BUFFER_SIZE: usize> {
    buffer: [(SonyIRRawSendableCommand, SonyIREncoderPause, u8); BUFFER_SIZE],
    buffer_index: usize,
    buffer_length: usize,
    state: SonyIREncoderState,
    next_command_needs_fill: bool,
    default_pause: SonyIREncoderPause,
}

impl<const BUFFER_SIZE: usize> SonyIREncoder<BUFFER_SIZE> {
    pub fn new() -> SonyIREncoder<BUFFER_SIZE> {
        // [todo] default_pause
        SonyIREncoder::with_commands_pause(SonyIREncoderPause::FromEnd(46 * (SystemTimer::TICKS_PER_SECOND / 1000))) /* commands_pause: 45ms */
    }

    pub fn with_commands_pause(default_pause: SonyIREncoderPause) -> SonyIREncoder<BUFFER_SIZE> {
        SonyIREncoder {
            buffer: [(SonyIRRawSendableCommand { data: 0, bits: 0 }, SonyIREncoderPause::FromStart(0), 0); BUFFER_SIZE],
            buffer_index: 0,
            buffer_length: 0,
            state: SonyIREncoderState::None,
            next_command_needs_fill: true,
            default_pause,
        }
    }

    pub fn is_transmitting(&self) -> bool {
        match self.state {
            SonyIREncoderState::TxPauseFromEnd(_) | SonyIREncoderState::TxPauseFromStart(_) => true,
            _ => false,
        }
    }

    fn can_start_with_state_update(&mut self) -> bool {
        match self.state {
            SonyIREncoderState::Paused(ready_at) => {
                if ready_at >= SystemTimer::now() {
                    self.state = SonyIREncoderState::None;
                    true
                } else {
                    false
                }
            }
            SonyIREncoderState::None => true,
            _ => false
        }
    }

    fn next_command_unchecked(&mut self) -> (SonyIRRawSendableCommand, SonyIREncoderPause) {
        let (command, pause, repeats) = self.buffer.index_mut(self.buffer_index);
        *repeats -= 1;

        if *repeats == 0 {
            self.buffer_length -= 1;
            self.buffer_index = (self.buffer_index + 1) % BUFFER_SIZE;
            self.next_command_needs_fill = true;
        } else {
            self.next_command_needs_fill = false;
        }

        (*command, *pause)
    }

    pub fn update<'a>(&mut self, mut rmt: PeripheralRef<'a, RMT>) -> Result<(), SonyIRError> {
        let peripheral_result = if self.is_transmitting() {
            let rmt_ch0_status = rmt_tx::rmt_ch0_is_done(rmt.reborrow());

            match rmt_ch0_status {
                Ok(true) | Err(()) => {
                    self.state = match self.state {
                        SonyIREncoderState::TxPauseFromEnd(pause_time) => SonyIREncoderState::Paused(SystemTimer::now() + pause_time),
                        SonyIREncoderState::TxPauseFromStart(ready_at) => SonyIREncoderState::Paused(ready_at),
                        _ => unreachable!(), /* self.is_transmitting() is true so other states are impossible */
                    }
                }
                _ => {}
            }

            match rmt_ch0_status {
                Ok(_) => Ok(()),
                Err(()) => Err(SonyIRError::RMTPeripheral),
            }
        } else {
            Ok(())
        };

        if self.buffer_length == 0 || !self.can_start_with_state_update() {
            return peripheral_result;
        }

        let fill = self.next_command_needs_fill;
        let (command, pause) = self.next_command_unchecked();

        if fill {
            sony_ir_ch0_fill_ram_raw(command);
        }

        rmt_tx::rmt_ch0_start(rmt.reborrow());
        self.state = match pause {
            SonyIREncoderPause::FromStart(pause_time) => SonyIREncoderState::TxPauseFromStart(SystemTimer::now() + pause_time),
            SonyIREncoderPause::FromEnd(pause_time) => SonyIREncoderState::TxPauseFromEnd(pause_time),
        };

        peripheral_result
    }

    fn send_non_immediatly_raw<'a>(&mut self, command: SonyIRRawSendableCommand, repeats: u8, pause: SonyIREncoderPause) -> Result<(), SonyIRError> {
        if self.buffer_length == BUFFER_SIZE {
            return Err(SonyIRError::EncoderBufferFull);
        }

        let buffer_next_index = (self.buffer_index + self.buffer_length) % BUFFER_SIZE;
        self.buffer[buffer_next_index] = (command, pause, repeats);
        self.buffer_length += 1;

        Ok(())
    }

    pub fn send_non_immediatly<'a>(&mut self, command: SonyIRCommand, repeats: u8, pause: Option<SonyIREncoderPause>) -> Result<(), SonyIRError> {
        if repeats == 0 {
            return Ok(());
        }

        let command = SonyIRRawSendableCommand::from_command(command).ok_or(SonyIRError::UnsendableCommand)?;

        self.send_non_immediatly_raw(command, repeats, pause.unwrap_or(self.default_pause))
    }

    pub fn send<'a>(&mut self, rmt: PeripheralRef<'a, RMT>, command: SonyIRCommand, mut repeats: u8, pause: Option<SonyIREncoderPause>) -> Result<(), SonyIRError> {
        if repeats == 0 {
            return Ok(());
        }

        let command = SonyIRRawSendableCommand::from_command(command).ok_or(SonyIRError::UnsendableCommand)?;
        let pause = pause.unwrap_or(self.default_pause);

        if self.buffer_length == 0 && self.can_start_with_state_update() {
            repeats -= 1;

            sony_ir_ch0_fill_ram_raw(command);
            rmt_tx::rmt_ch0_start(rmt);
            self.state = match pause {
                SonyIREncoderPause::FromStart(pause_time) => SonyIREncoderState::TxPauseFromStart(SystemTimer::now() + pause_time),
                SonyIREncoderPause::FromEnd(pause_time) => SonyIREncoderState::TxPauseFromEnd(pause_time),
            };

            if repeats == 0 {
                return Ok(());
            }
        }

        self.send_non_immediatly_raw(command, repeats, pause)
    }
}