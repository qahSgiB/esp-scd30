use core::mem::MaybeUninit;

use esp_hal::{clock::Clocks, gpio::{InputPin, Level, OutputOpenDrain, OutputPin, Pull}, i2c::Instance, peripheral::{Peripheral, PeripheralRef}, peripherals::{self, I2C0}};

use fugit::HertzU32;

use crate::interrupts::I2CInterruptStatus;



#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2CTransmissionError {
    Unknown(I2CInterruptStatus),
}

impl I2CTransmissionError {
    pub fn from_interrupt_flags(interrupt: I2CInterruptStatus) -> Option<I2CTransmissionError> {
        interrupt.is_error().then_some(I2CTransmissionError::Unknown(interrupt))
    }

    // TODO: maybe remove
    // pub fn from_interrupt_flags_unchecked(interrupt: I2CInterruptStatus) -> I2CTransmissionError {
    //     I2CTransmissionError::Unknown(interrupt)
    // }
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


pub fn setup<'a>( mut i2c: PeripheralRef<'a, I2C0>, freq: HertzU32, clocks: &Clocks) {
    // 0x10 is default value, overriding value computed by `i2c::Instance::set_frequency`
    i2c.setup(freq, clocks, Some(0x10)); // [todo] look into this

    i2c.fifo_conf().modify(|_, w| {
        w.nonfifo_en().clear_bit()
         .fifo_prt_en().clear_bit()
    });

    i2c.int_ena().modify(|_, w| {
        w.trans_complete().set_bit()
         .arbitration_lost().set_bit()
         .nack().set_bit()
         .time_out().set_bit()
         .scl_main_st_to().set_bit()
         .scl_st_to().set_bit()
    });
}

/// prepare pins for usage with i2c
pub fn setup_pins<'a, 'b, SCL, SDA>(
    scl_pin: impl Peripheral<P = SCL> + 'a,
    sda_pin: impl Peripheral<P = SDA> + 'b
) -> (OutputOpenDrain<'a, SCL>, OutputOpenDrain<'b, SDA>)
where
    SCL: OutputPin + InputPin,
    SDA: OutputPin + InputPin,
{
    // scl_pin
    //     .set_to_open_drain_output()
    //     .enable_input(true)
    //     .internal_pull_up(false)
    //     .connect_peripheral_to_output(OutputSignal::I2CEXT0_SCL)
    //     .connect_input_to_peripheral(InputSignal::I2CEXT0_SCL);

    // sda_pin
    //     .set_to_open_drain_output()
    //     .enable_input(true)
    //     .internal_pull_up(false)
    //     .connect_peripheral_to_output(OutputSignal::I2CEXT0_SDA)
    //     .connect_input_to_peripheral(InputSignal::I2CEXT0_SDA);

    // TODO: level ok?, enable input by default ok?, connect to peripheral
    let scl_pin = OutputOpenDrain::new(scl_pin, Level::High, Pull::None);
    let sda_pin = OutputOpenDrain::new(sda_pin, Level::High, Pull::None);

    let scl_num = 4;
    let sda_num = 5;

    // TODO
    // SAFETY: only scl and sda pins are accessed from following struct, and scl and sda pins are owned by this function ???
    let pac_gpio = unsafe { peripherals::GPIO::steal() };
    let pac_io_mux = unsafe { peripherals::IO_MUX::steal() };

    // SAFETY: bits valid according to esp32c6 docs

    pac_io_mux.gpio(scl_num).modify(|_, w| unsafe {
        w
            .fun_ie().bit(true) // enable input
            .mcu_sel().bits(1) // set alternate function to 1 - use gpio matrix
    });
    pac_gpio.func_out_sel_cfg(scl_num).modify(|_, w| unsafe {
        w.out_sel().bits(45) // connect output to gpio via gpio matrix
    });
    pac_gpio.func_in_sel_cfg(45).modify(|_, w| unsafe {
        w
            .sel().set_bit() // use gpio matrix for input
            .in_sel().bits(scl_num as u8) // connect input to gpio via gpio matrix
    });

    pac_io_mux.gpio(sda_num).modify(|_, w| unsafe {
        w
            .fun_ie().bit(true) // enable input
            .mcu_sel().bits(1) // set alternate function to 1 - use gpio matrix
    });
    pac_gpio.func_out_sel_cfg(sda_num).modify(|_, w| unsafe {
        w.out_sel().bits(46) // connect output to gpio via gpio matrix
    });
    pac_gpio.func_in_sel_cfg(46).modify(|_, w| unsafe {
        w
            .sel().set_bit() // use gpio matrix for input
            .in_sel().bits(sda_num as u8) // connect input to gpio via gpio matrix
    });

    (scl_pin, sda_pin)
}

pub fn reset_fifo(i2c: PeripheralRef<I2C0>) {
    i2c.fifo_conf().modify(|_, w| {
        w.tx_fifo_rst().set_bit()
         .rx_fifo_rst().set_bit()
    });

    i2c.fifo_conf().modify(|_, w| {
        w.tx_fifo_rst().clear_bit()
         .rx_fifo_rst().clear_bit()
    });
}

// TODO: should this be unsafe?
/// # Safety
/// 
/// `bytes.len() <= 31` - exp32-c6 I2C fifo has maximum capacity of 32 bytes and one byte is used for the address
pub unsafe fn prepare_write_unchecked(i2c: PeripheralRef<I2C0>, address: u8, bytes: &[u8]) {
    let commands = [
        I2CCommand::Start,
        I2CCommand::Write { ack_ckeck: true, ack_exp: false, len: (bytes.len() + 1) as u8 },
        I2CCommand::Stop,
    ];
    // SAFETY: `I2CCommand::into` creates valid command bits
    i2c.comd_iter().zip(commands.into_iter()).for_each(|(cmd_reg, cmd)| cmd_reg.write(|w| unsafe { w.command().bits(cmd.into()) }));

    i2c.data().write(|w| w.fifo_rdata().bits((address << 1) | 0));
    // SAFETY: any byte is valid for sending through i2c
    bytes.into_iter().for_each(|byte| i2c.data().write(|w| unsafe { w.fifo_rdata().bits(*byte) }));
}

/// # Safety
/// 
/// `len <= 32` - exp32-c6 I2C fifo has maximum capacity of 32 bytes
pub unsafe fn prepare_read_unchecked(i2c: PeripheralRef<I2C0>, address: u8, len: u8) {
    let commands = [
        I2CCommand::Start,
        I2CCommand::Write { ack_ckeck: true, ack_exp: false, len: 1 },
        I2CCommand::Read { ack: false, len: len - 1 },
        I2CCommand::Read { ack: true, len: 1 },
        I2CCommand::Stop,
    ];
    // SAFETY: `I2CCommand::into` creates valid command bits
    i2c.comd_iter().zip(commands.into_iter()).for_each(|(cmd_reg, cmd)| cmd_reg.write(|w| unsafe { w.command().bits(cmd.into()) }));

    // SAFETY: any byte is valid for sending through i2c
    i2c.data().write(|w| unsafe { w.fifo_rdata().bits((address << 1) | 1) });
}

pub fn start(i2c: PeripheralRef<I2C0>) {
    i2c.ctr().modify(|_, w| w.trans_start().set_bit());
}

/// # Safety
/// 
/// Same as `prepare_write_unchecked`, `bytes.len() <= 31`.
pub unsafe fn do_write(mut i2c: PeripheralRef<I2C0>, address: u8, bytes: &[u8]) {
    reset_fifo(i2c.reborrow());

    // SAFETY: checked by user
    unsafe { prepare_write_unchecked(i2c.reborrow(), address, bytes) };

    start(i2c.reborrow());
}

/// # Safety
/// 
/// Same as `prepare_read_unchecked`, `len <= 31`.
pub unsafe fn do_read(mut i2c: PeripheralRef<I2C0>, address: u8, len: u8) {
    reset_fifo(i2c.reborrow());

    // SAFETY: checked by user
    unsafe { prepare_read_unchecked(i2c.reborrow(), address, len) };

    start(i2c.reborrow());
}

pub fn read_response<const N: usize>(i2c: PeripheralRef<I2C0>) -> [u8; N] {
    let mut buffer = [MaybeUninit::uninit(); N];

    // TODO: check if there is enough data in fifo
    buffer.iter_mut().for_each(|b| {
        // no leak happens because there is no data in buffer
        b.write(i2c.data().read().fifo_rdata().bits());
    });

    // SAFETY: buffer is fully initialized by `for_each`
    buffer.map(|b| unsafe { MaybeUninit::assume_init(b) })
}