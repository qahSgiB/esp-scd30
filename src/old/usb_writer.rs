/// Wrapper around TX part of USB_DEVICE peripheral.
/// Provides a software buffer which enables non blocking writes of large data that would otherwise not fit into the hardware USB fifo in one write and cause busy wait.
/// 
/// Data flow: user data -> (write method) -> software buffer -> (update method) -> hardware USB fifo
/// 
/// # `UsbWriter`
/// This struct provides the software buffered writing.
/// It has two kind of methods for controling the software buffer and USB peripheral.
/// * writing methods handles writing of &[u8] into the software buffer
/// * updating methods handles moving data from software buffer into the hardware USB fifo
/// When you have some data which you want output to the usb, call one of the writing methods.
/// (So writing methods are something like println! if you want to use USB as way to print to the computer terminal).
/// Updating methods should be called regullary because writing methods don't move any data into the hardware USB fifo usually.
/// 
/// `UsbWriter` struct takes ownership of the USB_DEVICE peripheral.
/// That means after constructing `UsbWriter` you cannot use USB_DEVICE for anything else.
/// For example you cannot use it for RX.
/// This may be unwanted.
/// In that case you can use `UsbWriterBuffered`.
/// 
/// # `UsbWriterBuffered`
/// `UsbWriterBuffered` is somewhat simpler version of `UsbWriter` struct.
/// Simpler in a sense that `UsbWriterBuffered` doesn't own USB_DEVICE peripheral, instead it takes USB_DEVICE as an argument for (almost) all update and writing methods.
/// This way user can use `UsbWriter` without having to lose ownership of USB_DEVICE.
/// This also comes with some pottential risks.
/// When using `UsbWriterBuffered` and also using USB_DEVICE for some other USB writes (writes not by `UsbWriterBuffered`) it is highly possible that the data on the other end of the USB will appear in wierd unpredictable order.
/// This is caused by the fact that writing methods of `UsbWriterBuffered` doesn't immediatly outputs the data on the USB but instead they place the data in software buffer.
/// So simultaneous usage of `UsbWriterBuffered` and TX part of USB_DEVICE is highly discouraged.
/// On the other there is no problem with using `UsbWriterBuffered` and RX part of USB_DEVICE at the same time.
/// 
/// # Methods
/// Both `UsbWriter` and `UsbWriterBuffered` provides methods described in this section.
/// 
/// Write methods:
/// * write_bytes_non_immediate - writes data provided by the user into the software buffer
/// * write_bytes_try_immediate - same as `write_bytes_non_immediate` but when it is possible it writes data directly into the hardware USB fifo (possible means that there is nothing esle in the software buffer and that there is space in the USB fifo)
/// 
/// Update methods:
/// * update_without_blocking - writes from software buffer into the USB fifo until the USB fifo is full
/// * update_blocking - writes from software buffer into the USB fifo while there is less than `minimum_empty_space` empty space in the software buffer in a blocking way, which means that when the USB fifo is full it waits in busy loop for free USB fifo
/// * update_blocking_empty - same as `update_blocking` but doesn't use the `minimum_empty_space` but it empyies the whole buffer in the blocking way

// [todo] unify terminolgy: USB_DEVICE peripheral, USB fifo
// [todo] document usage if write!
// [todo] exampe usage



use core::fmt::Write;

use esp_hal::{
    peripheral::{Peripheral, PeripheralRef},
    peripherals::USB_DEVICE,
    systimer::SystemTimer
};



fn usb_fifo_free<'a>(usb: PeripheralRef<'a, USB_DEVICE>) -> bool {
    usb.ep1_conf().read().serial_in_ep_data_free().bit_is_set()
}

fn usb_write<'a>(usb: PeripheralRef<'a, USB_DEVICE>, byte: u8) -> () {
    usb.ep1().write(|w| w.rdwr_byte().variant(byte));
}

fn usb_flush<'a>(usb: PeripheralRef<'a, USB_DEVICE>) -> () {
    usb.ep1_conf().write(|w| w.wr_done().set_bit());
}




#[derive(Debug)]
pub enum UsbWriterError {
    Overflow(usize),
}

#[derive(Debug)]
pub enum UsbUpdateResult {
    Ok,
    TimeOut,
}



pub struct UsbWriterBuffered<const BUFFER_SIZE: usize> {
    buffer: [u8; BUFFER_SIZE],
    buffer_index: usize,
    buffer_length: usize,
    timeout: bool,
    minimum_empty_space: usize,
    timeout_treshold: u64,
}

impl<const BUFFER_SIZE: usize> UsbWriterBuffered<BUFFER_SIZE> {
    pub fn new(minimum_empty_space: usize, timeout_treshold: u64) -> UsbWriterBuffered<BUFFER_SIZE> {
        UsbWriterBuffered::<BUFFER_SIZE> {
            buffer: [0; BUFFER_SIZE],
            buffer_index: 0,
            buffer_length: 0,
            timeout: false,
            minimum_empty_space,
            timeout_treshold,
        }
    }

    pub fn builder() -> UsbWriterBufferedBuilder<BUFFER_SIZE> {
        UsbWriterBufferedBuilder::new()
    }

    pub fn with_usb<'a>(self, usb: impl Peripheral<P = USB_DEVICE> + 'a) -> UsbWriter<'a, BUFFER_SIZE> {
        UsbWriter {
            writer: self,
            usb: usb.into_ref()
        }
    }

    pub fn buffer_empty(&self) -> bool {
        self.buffer_length == 0
    }

    fn buffer_next_unchecked(&mut self) {
        self.buffer_length -= 1;
        self.buffer_index = (self.buffer_index + 1) % BUFFER_SIZE;
    }

    fn buffer_pop_write_unchecked<'a>(&mut self, usb: PeripheralRef<'a, USB_DEVICE>) {
        let byte = self.buffer[self.buffer_index];
        self.buffer_next_unchecked();

        usb_write(usb, byte);
    }

    fn buffer_extend_continous<'a>(&mut self, from: usize, to: usize, bytes: &'a [u8]) -> &'a [u8] {
        let size = to - from;

        if bytes.len() < size {
            self.buffer[from..(from + bytes.len())].copy_from_slice(bytes);
            self.buffer_length += bytes.len();
            &[]
        } else {
            self.buffer[from..to].copy_from_slice(&bytes[..size]);
            self.buffer_length += size;
            &bytes[size..]
        }
    }

    fn buffer_extend(&mut self, bytes: &[u8]) -> usize {
        if self.buffer_length == BUFFER_SIZE {
            return bytes.len();
        }

        let buffer_continue_index = (self.buffer_index + self.buffer_length) % BUFFER_SIZE;

        if buffer_continue_index < self.buffer_index {
            self.buffer_extend_continous(buffer_continue_index, self.buffer_index, bytes).len()
        } else {
            let bytes_rem = self.buffer_extend_continous(buffer_continue_index, BUFFER_SIZE, bytes);

            if bytes_rem.len() > 0 {
                self.buffer_extend_continous(0, self.buffer_index, bytes_rem).len()
            } else {
                0
            }
        }
    }

    pub fn update_without_blocking<'a>(&mut self, mut usb: PeripheralRef<'a, USB_DEVICE>) {
        let mut emptied = true;

        while !self.buffer_empty() {
            if !usb_fifo_free(usb.reborrow()) {
                emptied = false;
                break;
            }

            self.buffer_pop_write_unchecked(usb.reborrow());
        }

        if emptied {
            usb_flush(usb.reborrow());
        }
    }

    fn update_blocking_param<'a>(&mut self, mut usb: PeripheralRef<'a, USB_DEVICE>, max_buffer_length: usize) -> UsbUpdateResult {
        if self.buffer_empty() {
            return UsbUpdateResult::Ok;
        }

        /* tries writing first byte to check if there is still timeout */
        if usb_fifo_free(usb.reborrow()) {
            self.buffer_pop_write_unchecked(usb.reborrow());
            self.timeout = false;
        }

        if self.timeout {
            return UsbUpdateResult::TimeOut;
        }

        while self.buffer_length > max_buffer_length {
            /* tries writing wihtout waiting */
            if usb_fifo_free(usb.reborrow()) {
                self.buffer_pop_write_unchecked(usb.reborrow());
                continue;
            }

            /* wait for fifo free or timeout */
            let byte = self.buffer[self.buffer_index];
            let byte_start_time = SystemTimer::now();

            while !usb_fifo_free(usb.reborrow()) {
                if SystemTimer::now() - byte_start_time >= self.timeout_treshold {
                    return UsbUpdateResult::TimeOut;
                }
            }

            usb_write(usb.reborrow(), byte);
            self.buffer_next_unchecked();
        }

        self.update_without_blocking(usb);

        UsbUpdateResult::Ok
    }

    pub fn update_blocking<'a>(&mut self, usb: PeripheralRef<'a, USB_DEVICE>) -> UsbUpdateResult {
        self.update_blocking_param(usb, BUFFER_SIZE - self.minimum_empty_space)
    }

    pub fn update_blocking_empty<'a>(&mut self, usb: PeripheralRef<'a, USB_DEVICE>) -> UsbUpdateResult {
        self.update_blocking_param(usb, 0)
    }

    pub fn write_bytes_try_immediate<'a>(&mut self, mut usb: PeripheralRef<'a, USB_DEVICE>, bytes: &[u8]) -> Result<(), UsbWriterError> {
        let overflow = if self.buffer_empty() {
            let mut bytes_index = 0;
            let mut emptied = true;

            while bytes_index < bytes.len() {
                if !usb_fifo_free(usb.reborrow()) {
                    emptied = false;
                    break;
                }
                
                usb_write(usb.reborrow(), bytes[bytes_index]);
                bytes_index += 1;
            }

            if emptied {
                usb_flush(usb.reborrow());
            }

            if bytes_index != 0 {
                self.timeout = false;
            }

            if bytes_index < bytes.len() {
                self.buffer_extend(&bytes[bytes_index..])
            } else {
                0
            }
        } else {
            self.buffer_extend(bytes)
        };

        match overflow {
            0 => Ok(()),
            n => Err(UsbWriterError::Overflow(n)),
        }
    }

    pub fn write_bytes_non_immediate(&mut self, bytes: &[u8]) -> Result<(), UsbWriterError> {
        let overflow = self.buffer_extend(bytes);

        match overflow {
            0 => Ok(()),
            n => Err(UsbWriterError::Overflow(n)),
        }
    }

    pub fn twni(&mut self) -> TemporaryNonImmiediateUsbWriter<BUFFER_SIZE> {
        TemporaryNonImmiediateUsbWriter {
            writer: self,
        }
    }

    pub fn twti<'a, 'b>(&'a mut self, usb: PeripheralRef<'b, USB_DEVICE>) -> TemporaryTryImmiediateUsbWriter<'a, 'b, BUFFER_SIZE> {
        TemporaryTryImmiediateUsbWriter {
            writer: self,
            usb
        }
    }
}

impl<const BUFFER_SIZE: usize> Default for UsbWriterBuffered<BUFFER_SIZE> {
    fn default() -> Self {
        UsbWriterBufferedBuilder::new().build()
    }
}


#[derive(Default, Clone, Copy)]
pub struct UsbWriterBufferedBuilder<const BUFFER_SIZE: usize = 1024> {
    minimum_empty_space: Option<usize>,
    timeout_treshold: Option<u64>,
}

impl<const BUFFER_SIZE: usize> UsbWriterBufferedBuilder<BUFFER_SIZE> {
    pub const DEFAULT_MINIMUM_EMPTY_SPACE: usize = BUFFER_SIZE * 3 / 4;
    pub const DEFAULT_TIMEOUT_TRESHOLD: u64 = SystemTimer::TICKS_PER_SECOND / 1_000;


    pub fn new() -> Self {
        Default::default()
    }

    pub fn minimum_empty_space(&mut self, minimum_empty_space: usize) -> Self {
        self.minimum_empty_space = Some(minimum_empty_space);
        *self
    }

    pub fn timeout_treshold(&mut self, timeout_treshold: u64) -> Self {
        self.timeout_treshold = Some(timeout_treshold);
        *self
    }

    pub fn build(&mut self) -> UsbWriterBuffered<BUFFER_SIZE> {
        UsbWriterBuffered::<BUFFER_SIZE>::new(
            self.minimum_empty_space.unwrap_or(Self::DEFAULT_MINIMUM_EMPTY_SPACE),
            self.timeout_treshold.unwrap_or(Self::DEFAULT_TIMEOUT_TRESHOLD),
        )
    }
}


pub struct TemporaryNonImmiediateUsbWriter<'a, const BUFFER_SIZE: usize> {
    writer: &'a mut UsbWriterBuffered<BUFFER_SIZE>,
}

impl<'a, const BUFFER_SIZE: usize> Write for TemporaryNonImmiediateUsbWriter<'a, BUFFER_SIZE> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.writer.write_bytes_non_immediate(s.as_bytes()).map_err(|_| core::fmt::Error)
    }
}

// [todo] lifetimes
pub struct TemporaryTryImmiediateUsbWriter<'a, 'b, const BUFFER_SIZE: usize> {
    writer: &'a mut UsbWriterBuffered<BUFFER_SIZE>,
    usb: PeripheralRef<'b, USB_DEVICE>,
}

impl<'a, 'b, const BUFFER_SIZE: usize> Write for TemporaryTryImmiediateUsbWriter<'a, 'b, BUFFER_SIZE> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.writer.write_bytes_try_immediate(self.usb.reborrow(), s.as_bytes()).map_err(|_| core::fmt::Error)
    }
}


// [todo] builder
pub struct UsbWriter<'a, const BUFFER_SIZE: usize> {
    writer: UsbWriterBuffered<BUFFER_SIZE>,
    usb: PeripheralRef<'a, USB_DEVICE>,
}

impl<'a, const BUFFER_SIZE: usize> UsbWriter<'a, BUFFER_SIZE> {
    pub fn buffer_empty(&self) -> bool { self.writer.buffer_empty() }
    pub fn update_without_blocking(&mut self) { self.writer.update_without_blocking(self.usb.reborrow()) }
    pub fn update_blocking(&mut self) -> UsbUpdateResult { self.writer.update_blocking(self.usb.reborrow()) }
    pub fn update_blocking_empty(&mut self) -> UsbUpdateResult { self.writer.update_blocking_empty(self.usb.reborrow()) }
    pub fn write_bytes_try_immediate(&mut self, bytes: &[u8]) -> Result<(), UsbWriterError> { self.writer.write_bytes_try_immediate(self.usb.reborrow(), bytes) }
    pub fn write_bytes_non_immediate(&mut self, bytes: &[u8]) -> Result<(), UsbWriterError> { self.writer.write_bytes_non_immediate(bytes) }
    pub fn twni(&mut self) -> TemporaryNonImmiediateUsbWriter<BUFFER_SIZE> { self.writer.twni() }
    pub fn twti(&mut self) -> TemporaryTryImmiediateUsbWriter<BUFFER_SIZE> { self.writer.twti(self.usb.reborrow()) }
}