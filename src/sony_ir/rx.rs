use super::SonyIRRawCommand;


struct SonyIRPulseRanges {
    short_min: u64,
    short_max: u64,
    mid_min: u64,
    mid_max: u64,
    long_min: u64,
    long_max: u64,
}

impl SonyIRPulseRanges {
    // fn new(range_div: u64, range_num: u64) -> SonyIRPulseRanges {
    //     SonyIRPulseRanges::with_short_min(range_div, range_num, range_div, range_num)
    // }

    fn with_short_min(range_div: u64, range_num: u64, short_min_div: u64, short_min_num: u64) -> SonyIRPulseRanges {
        SonyIRPulseRanges {
            short_min: SonyIRDeltaDecoder::PULSE_SHORT * (short_min_div - short_min_num) / short_min_div,
            short_max: SonyIRDeltaDecoder::PULSE_SHORT * (range_div + range_num) / range_div,
            mid_min: SonyIRDeltaDecoder::PULSE_MID * (range_div - range_num) / range_div,
            mid_max: SonyIRDeltaDecoder::PULSE_MID * (range_div + range_num) / range_div,
            long_min: SonyIRDeltaDecoder::PULSE_LONG * (range_div - range_num) / range_div,
            long_max: SonyIRDeltaDecoder::PULSE_LONG * (range_div + range_num) / range_div,
        }
    }

    fn is_short(&self, delta: u64) -> bool {
        self.short_min <= delta && delta <= self.short_max
    }

    // fn is_mid(&self, delta: u64) -> bool {
    //     self.mid_min <= delta && delta <= self.mid_max
    // }

    fn is_long(&self, delta: u64) -> bool {
        self.long_min <= delta && delta <= self.long_max
    }
}


#[derive(Debug, Clone, Copy)]
pub enum SonyIRError {
    Unknown, // [todo] error details
}

enum SonyIRDecoderState {
    None,
    StartWaitingLong,
    StartWaitingShort,
    Recieving {
        data: u32,
        bit: u8,
        waiting_zero: bool,
    },
    Error(SonyIRError),
}

pub struct SonyIRDeltaDecoder {
    ranges: SonyIRPulseRanges,
    state: SonyIRDecoderState,
}

impl SonyIRDeltaDecoder {
    const PULSE_SHORT: u64 = 600 * 16;
    const PULSE_MID: u64 = 1200 * 16;
    const PULSE_LONG: u64 = 2400 * 16;


    pub fn new() -> SonyIRDeltaDecoder {
        SonyIRDeltaDecoder::with_range(3, 1)
    }

    pub fn with_range(pulse_range_div: u64, pulse_range_num: u64) -> SonyIRDeltaDecoder {
        SonyIRDeltaDecoder {
            ranges: SonyIRPulseRanges::with_short_min(pulse_range_div, pulse_range_num, 2, 1),
            state: SonyIRDecoderState::None,
        }
    }

    /* user should reset timer after calling this function (when function returns Ok) */
    pub fn pulse(&mut self, delta: u64) -> Result<(), SonyIRError> {
        match self.state {
            SonyIRDecoderState::None => {
                self.state = SonyIRDecoderState::StartWaitingLong;
            },
            SonyIRDecoderState::StartWaitingLong => {
                if self.ranges.is_long(delta) {
                    self.state = SonyIRDecoderState::StartWaitingShort;
                } else {
                    self.state = SonyIRDecoderState::Error(SonyIRError::Unknown); /* unexpected pulse (starting long expected) */
                }
            }
            SonyIRDecoderState::StartWaitingShort => {
                if self.ranges.is_short(delta) {
                    self.state = SonyIRDecoderState::Recieving {
                        data: 0,
                        bit: 0,
                        waiting_zero: false,
                    };
                } else {
                    self.state = SonyIRDecoderState::Error(SonyIRError::Unknown); /* unexpected pulse (start finishing zero expected) */
                }
            },
            SonyIRDecoderState::Recieving { ref mut data, ref mut waiting_zero, ref mut bit } => {
                if *waiting_zero {
                    if self.ranges.is_short(delta) {
                        *waiting_zero = false;

                        if *bit == 19 {
                            self.state = SonyIRDecoderState::Error(SonyIRError::Unknown); /* maximum bit count reached */
                        } else {
                            *bit += 1;
                        }
                    } else {
                        self.state = SonyIRDecoderState::Error(SonyIRError::Unknown); /* unexpected pulse (recieving finishing zero expected) */
                    }
                } else {
                    if delta < self.ranges.short_min {
                        self.state = SonyIRDecoderState::Error(SonyIRError::Unknown); /* unexpected pulse (recieving zero or one expected) */
                    } else if delta <= self.ranges.short_max {
                        *waiting_zero = true;
                    } else if delta < self.ranges.mid_min {
                        self.state = SonyIRDecoderState::Error(SonyIRError::Unknown); /* unexpected pulse (recieving zero or one expected) */
                    } else if delta <= self.ranges.mid_max {
                        *data |= 1u32 << *bit;
                        *waiting_zero = true;
                    } else {
                        self.state = SonyIRDecoderState::Error(SonyIRError::Unknown); /* unexpected pulse (recieving finishing zero expected) */
                    }
                }
            },
            SonyIRDecoderState::Error(_) => {},
        }

        match self.state {
            SonyIRDecoderState::Error(error) => Err(error),
            _ => Ok(())
        }
    }

    pub fn timeout(&mut self) -> Result<SonyIRRawCommand, SonyIRError> {
        let result = match self.state {
            SonyIRDecoderState::Recieving { data, bit, waiting_zero } => {
                let bits = bit + 1;
                if waiting_zero && (bits == 12 || bits == 15 || bits == 20) {
                    Ok(SonyIRRawCommand { data, bits })
                } else {
                    Err(SonyIRError::Unknown) /* finished at invalid point | invalid bit count recieved */
                }
            },
            SonyIRDecoderState::Error(err) => Err(err),
            _ => Err(SonyIRError::Unknown) /* finished at invalid point */
        };

        self.reset();

        result
    }

    pub fn reset(&mut self) {
        self.state = SonyIRDecoderState::None;
    }
}


#[derive(Debug, Clone, Copy)]
pub enum SonyIREvent {
    TimeOut,
    Pulse(u64),
}


pub struct SonyIRDecoder {
    decoder: SonyIRDeltaDecoder,
    last_pulse: u64,
}

impl SonyIRDecoder {
    pub fn new() -> SonyIRDecoder {
        SonyIRDecoder {
            decoder: SonyIRDeltaDecoder::new(),
            last_pulse: 0,
        }
    }

    pub fn with_range(pulse_range_div: u64, pulse_range_num: u64) -> SonyIRDecoder {
        SonyIRDecoder {
            decoder: SonyIRDeltaDecoder::with_range(pulse_range_div, pulse_range_num),
            last_pulse: 0,
        }
    }

    pub fn update(&mut self, event: Option<SonyIREvent>) -> Result<Option<SonyIRRawCommand>, SonyIRError> {
        match event {
            Some(SonyIREvent::Pulse(ir_pulse)) => {
                let ir_delta = ir_pulse - self.last_pulse;
                self.last_pulse = ir_pulse;

                self.decoder.pulse(ir_delta).map(|_| None)
            },
            Some(SonyIREvent::TimeOut) => {
                self.decoder.timeout().map(Some)
            },
            None => { Ok(None) },
        }
    }
}