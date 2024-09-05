pub mod rx;
pub mod tx;



#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct SonyIRRawCommand {
    pub data: u32,
    pub bits: u8,
}


#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum SonyIRCommand {
    V12 { address: u8, command: u8 },
    V15 { address: u8, command: u8 },
    Raw(SonyIRRawCommand),
}

// impl From<SonyIRRawCommand> for SonyIRCommand {
//     fn from(value: SonyIRRawCommand) -> SonyIRCommand {
//         match value.bits {
//             12 => SonyIRCommand::V12 { address: (value.data >> 7) as u8, command: (value.data & 0b0111_1111) as u8 },
//             15 => SonyIRCommand::V15 { address: (value.data >> 7) as u8, command: (value.data & 0b0111_1111) as u8 },
//             _ => SonyIRCommand::Unknown(value),
//         }
//     }
// }

impl SonyIRCommand {
    pub fn from_raw(raw: &SonyIRRawCommand) -> SonyIRCommand { // [todo] remove ref
        match raw.bits {
            12 => SonyIRCommand::V12 { address: (raw.data >> 7) as u8, command: (raw.data & 0b0111_1111) as u8 },
            15 => SonyIRCommand::V15 { address: (raw.data >> 7) as u8, command: (raw.data & 0b0111_1111) as u8 },
            _ => SonyIRCommand::Raw(*raw),
        }
    }
}

impl SonyIRRawCommand {
    pub fn from_command(command: SonyIRCommand) -> SonyIRRawCommand {
        match command {
            SonyIRCommand::V12 { address, command } => SonyIRRawCommand {
                data: (((address & 0b0001_1111) as u32) << 7) | ((command & 0b0111_1111) as u32),
                bits: 12
            },
            SonyIRCommand::V15 { address, command } => SonyIRRawCommand {
                data: (((address & 0b1111_1111) as u32) << 7) | ((command & 0b0111_1111) as u32),
                bits: 12
            },
            SonyIRCommand::Raw(raw) => raw,
        }
    }
}