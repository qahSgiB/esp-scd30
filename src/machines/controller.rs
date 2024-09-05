use core::{cmp::Ordering, fmt::Write};

use esp_hal::timer::systimer::SystemTimer;

use crate::{ring_buffer::{Overwrite, RingBuffer}, sdc::RawMeasurment};



#[derive(Debug, Clone, Copy)]
enum ParseFloatE3Error {
    Negative,
    TooSmall,
    TooBig,
}

fn parse_float_e3(f: u32) -> Result<u32, ParseFloatE3Error> {
    let sign = (f >> 31) as u8;
    let exp = (f >> 23) as u8;
    let frac = f & 0x3FFFFF;

    if sign == 1 {
        return Err(ParseFloatE3Error::Negative);
    }

    let exp = exp.checked_sub(124).ok_or(ParseFloatE3Error::TooSmall)?;
    if exp > 31 {
        return Err(ParseFloatE3Error::TooBig);
    }

    let mut dec = match exp.cmp(&23) {
        Ordering::Less => frac >> (23 - exp),
        Ordering::Greater => frac << (exp - 23),
        Ordering::Equal => frac,
    };
    dec |= 1u32 << exp;

    dec.checked_mul(125).ok_or(ParseFloatE3Error::TooBig)
}



struct TimedMeasurment {
    measurment: RawMeasurment,
    at: u64,
}


pub struct Controller<const N: usize> {
    measurments: RingBuffer<TimedMeasurment, N, Overwrite>,
    pending_measurment: Option<RawMeasurment>,
}

impl<const N: usize> Controller<N> {
    pub fn new() -> Self {
        Self {
            measurments: RingBuffer::new(),
            pending_measurment: None,
        }
    }

    pub fn update(&mut self, usb_writer: &mut impl Write) -> bool {
        if let Some(measurment) = self.pending_measurment.take() {
            let co2 = parse_float_e3(u32::from_be_bytes(measurment.co2));
            let temperature = parse_float_e3(u32::from_be_bytes(measurment.temperature));
            let humidity = parse_float_e3(u32::from_be_bytes(measurment.humidity));

            if let Err(e) = co2 {
                let _ = writeln!(usb_writer, "cannot parse co2 : {:?}", e);
            }
            if let Err(e) = temperature {
                let _ = writeln!(usb_writer, "cannot parse temperature : {:?}", e);
            }
            if let Err(e) = humidity {
                let _ = writeln!(usb_writer, "cannot parse humidity : {:?}", e);
            }

            if let Ok(co2) = co2 && let Ok(temperature) = temperature && let Ok(humidity) = humidity {
                let _ = writeln!(usb_writer, "co2 : {}.{} ppm", co2 / 1000, co2 % 1000);
                let _ = writeln!(usb_writer, "temperature : {}.{} °C", temperature / 1000, temperature % 1000);
                let _ = writeln!(usb_writer, "humidity : {}.{} %", humidity / 1000, humidity % 1000);
            }

            let now = SystemTimer::now();
            self.measurments.push_back(TimedMeasurment { measurment, at: now });

            // TODO: process measurment

            true
        } else {
            false
        }
    }

    pub fn on_measurment(&mut self, measurment: RawMeasurment) {
        self.pending_measurment = Some(measurment);
    }
}