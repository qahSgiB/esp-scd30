use core::{mem::MaybeUninit, num::NonZeroU16};

use esp_hal::{peripheral::PeripheralRef, peripherals::I2C0};

use fugit::SecsDurationU32;

use crate::pac_utils::i2c as i2c_utils;



pub mod machines;



const CRC_TABLE: [u8; 256] = [
    0x00, 0x31, 0x62, 0x53, 0xc4, 0xf5, 0xa6, 0x97, 0xb9, 0x88, 0xdb, 0xea, 0x7d, 0x4c, 0x1f, 0x2e,
    0x43, 0x72, 0x21, 0x10, 0x87, 0xb6, 0xe5, 0xd4, 0xfa, 0xcb, 0x98, 0xa9, 0x3e, 0x0f, 0x5c, 0x6d,
    0x86, 0xb7, 0xe4, 0xd5, 0x42, 0x73, 0x20, 0x11, 0x3f, 0x0e, 0x5d, 0x6c, 0xfb, 0xca, 0x99, 0xa8,
    0xc5, 0xf4, 0xa7, 0x96, 0x01, 0x30, 0x63, 0x52, 0x7c, 0x4d, 0x1e, 0x2f, 0xb8, 0x89, 0xda, 0xeb,
    0x3d, 0x0c, 0x5f, 0x6e, 0xf9, 0xc8, 0x9b, 0xaa, 0x84, 0xb5, 0xe6, 0xd7, 0x40, 0x71, 0x22, 0x13,
    0x7e, 0x4f, 0x1c, 0x2d, 0xba, 0x8b, 0xd8, 0xe9, 0xc7, 0xf6, 0xa5, 0x94, 0x03, 0x32, 0x61, 0x50,
    0xbb, 0x8a, 0xd9, 0xe8, 0x7f, 0x4e, 0x1d, 0x2c, 0x02, 0x33, 0x60, 0x51, 0xc6, 0xf7, 0xa4, 0x95,
    0xf8, 0xc9, 0x9a, 0xab, 0x3c, 0x0d, 0x5e, 0x6f, 0x41, 0x70, 0x23, 0x12, 0x85, 0xb4, 0xe7, 0xd6,
    0x7a, 0x4b, 0x18, 0x29, 0xbe, 0x8f, 0xdc, 0xed, 0xc3, 0xf2, 0xa1, 0x90, 0x07, 0x36, 0x65, 0x54,
    0x39, 0x08, 0x5b, 0x6a, 0xfd, 0xcc, 0x9f, 0xae, 0x80, 0xb1, 0xe2, 0xd3, 0x44, 0x75, 0x26, 0x17,
    0xfc, 0xcd, 0x9e, 0xaf, 0x38, 0x09, 0x5a, 0x6b, 0x45, 0x74, 0x27, 0x16, 0x81, 0xb0, 0xe3, 0xd2,
    0xbf, 0x8e, 0xdd, 0xec, 0x7b, 0x4a, 0x19, 0x28, 0x06, 0x37, 0x64, 0x55, 0xc2, 0xf3, 0xa0, 0x91,
    0x47, 0x76, 0x25, 0x14, 0x83, 0xb2, 0xe1, 0xd0, 0xfe, 0xcf, 0x9c, 0xad, 0x3a, 0x0b, 0x58, 0x69,
    0x04, 0x35, 0x66, 0x57, 0xc0, 0xf1, 0xa2, 0x93, 0xbd, 0x8c, 0xdf, 0xee, 0x79, 0x48, 0x1b, 0x2a,
    0xc1, 0xf0, 0xa3, 0x92, 0x05, 0x34, 0x67, 0x56, 0x78, 0x49, 0x1a, 0x2b, 0xbc, 0x8d, 0xde, 0xef,
    0x82, 0xb3, 0xe0, 0xd1, 0x46, 0x77, 0x24, 0x15, 0x3b, 0x0a, 0x59, 0x68, 0xff, 0xce, 0x9d, 0xac
];

const CRC_INIT_MAGIC: u8 = 0xac;



/// Computes crc for 2 bytes.
/// `b2` is MSB and `b1` is LSB.
pub fn compute_crc(b2: u8, b1: u8) -> u8 {
    let t = CRC_TABLE[b2 as usize] ^ CRC_INIT_MAGIC ^ b1;
    CRC_TABLE[t as usize]
}

pub fn check_crc(b2: u8, b1: u8, crc: u8) -> bool {
    compute_crc(b2, b1) == crc
}



pub const DEFAULT_ADDRESS: u8 = 0x61;



#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SDCReadResponseError {
    CRCCheckFailed,
    InvalidFormat,
}



#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SDCSetCommand {
    SetDelta {
        delta: SecsDurationU32, // TODO: check interval constraints
    },
    Start {
        pressure: Option<NonZeroU16>, // TODO: check interval constraints
    },
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SDCGetCommand {
    IsReady,
    Measurment,
}


fn u16_into_param_bytes(v: u16) -> (u8, u8, u8) {
    let b2 = (v >> 8) as u8;
    let b1 = v as u8;
    let crc = compute_crc(b2, b1);
    (b2, b1, crc)
}

pub fn set_command_write(i2c: PeripheralRef<I2C0>, command: SDCSetCommand) {
    match command {
        SDCSetCommand::SetDelta { delta } => {
            let c = (0x46, 0x00);
            let p1 = u16_into_param_bytes(delta.to_secs() as u16);
            let bytes = [c.0, c.1, p1.0, p1.1, p1.2];

            // SAFETY: number of bytes is less then or equal to 31
            unsafe { i2c_utils::do_write(i2c, DEFAULT_ADDRESS, &bytes) };
        },
        SDCSetCommand::Start { pressure } => {
            let c = (0x00, 0x10);
            let p1 = u16_into_param_bytes(pressure.map_or(0, NonZeroU16::get));
            let bytes = [c.0, c.1, p1.0, p1.1, p1.2];
            // SAFETY: number of bytes is less then or equal to 31
            unsafe { i2c_utils::do_write(i2c, DEFAULT_ADDRESS, &bytes) };
        },
    }
}

pub fn get_command_write(i2c: PeripheralRef<I2C0>, command: SDCGetCommand) {
    match command {
        SDCGetCommand::IsReady => {
            let bytes = [0x02, 0x02];
            // SAFETY: number of bytes is less then or equal to 31
            unsafe { i2c_utils::do_write(i2c, DEFAULT_ADDRESS, &bytes) };
        },
        SDCGetCommand::Measurment => {
            let bytes = [0x03, 0x00];
            // SAFETY: number of bytes is less then or equal to 31
            unsafe { i2c_utils::do_write(i2c, DEFAULT_ADDRESS, &bytes) };
        },
    }
}

pub fn get_command_read(i2c: PeripheralRef<I2C0>, command: SDCGetCommand) {
    match command {
        SDCGetCommand::IsReady => {
            // SAFETY: `len <= 31`
            unsafe { i2c_utils::do_read(i2c, DEFAULT_ADDRESS, 3) };
        },
        SDCGetCommand::Measurment => {
            // SAFETY: `len <= 31`
            unsafe { i2c_utils::do_read(i2c, DEFAULT_ADDRESS, 3 * 6) };
        }
    }
}



pub fn read_response_param(i2c: PeripheralRef<I2C0>) -> Result<[u8; 2], SDCReadResponseError> {
    let [b2, b1, crc] = i2c_utils::read_response::<3>(i2c);

    if check_crc(b2, b1, crc) {
        Ok([b2, b1]) // TODO: is this correct?
    } else {
        Err(SDCReadResponseError::CRCCheckFailed)
    }
}

pub fn read_response_params<const N: usize>(mut i2c: PeripheralRef<I2C0>) -> Result<[[u8; 2]; N], SDCReadResponseError> {
    let mut buffer = [MaybeUninit::uninit(); N];

    buffer.iter_mut().try_for_each(|b| -> Result<(), SDCReadResponseError> {
        let param = read_response_param(i2c.reborrow())?;
        b.write(param);
        Ok(())
    })?;

    // SAFETY: if `try_for_each` did not fail buffer is initialized
    Ok(buffer.map(|b| unsafe { MaybeUninit::assume_init(b) }))
}


#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawMeasurment {
    pub co2: [u8; 4],
    pub temperature: [u8; 4],
    pub humidity: [u8; 4],
}

impl RawMeasurment {
    /// this method doesn't perform any check whether data is correct format (`f32`) and whether it is in valid range (specified by SDC30 documentation)
    pub fn from_sdc_response(bytes: [[u8; 2]; 6]) -> RawMeasurment {
        RawMeasurment {
            co2:         [bytes[0][0], bytes[0][1], bytes[1][0], bytes[1][1]],
            temperature: [bytes[2][0], bytes[2][1], bytes[3][0], bytes[3][1]],
            humidity:    [bytes[4][0], bytes[4][1], bytes[5][0], bytes[5][1]],
        }
    }
}


pub fn read_response_is_ready(i2c: PeripheralRef<I2C0>) -> Result<bool, SDCReadResponseError> {
    read_response_param(i2c).and_then(|bytes| {
        match bytes {
            [0, 0] => Ok(false),
            [0, 1] => Ok(true),
            _ => Err(SDCReadResponseError::InvalidFormat),
        }
    })
}

pub fn read_response_measurment(i2c: PeripheralRef<I2C0>) -> Result<RawMeasurment, SDCReadResponseError> {
    read_response_params::<6>(i2c).map(RawMeasurment::from_sdc_response)
}