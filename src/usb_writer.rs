use core::fmt::Write;


use esp_hal::{interrupt::Priority, peripheral::{Peripheral, PeripheralRef}, peripherals::USB_DEVICE, timer::systimer::SystemTimer};


use crate::{
    interrupts::{self, USBInterruptStatus},
    qq_alarm_queue::QQAlarmQueue,
    ring_buffer::{Ignore, RingBuffer, RingBufferError}
};




pub trait UsbWriter {
    fn write(&mut self, bytes: &[u8]) -> Result<(), RingBufferError>;
    fn is_timeouted(&self) -> bool; // TODO: should this be in this trait
}



#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TimeoutState {
    None,
    Pending(u64), // start at
    Active(usize), // qq alarm id
    Timeout,
}


/// usb writer, which uses ring buffer to buffer data
pub struct RingBufferUsbWriter<'a, const BUFFER_SIZE: usize> {
    usb: PeripheralRef<'a, USB_DEVICE>,
    buffer: RingBuffer<u8, BUFFER_SIZE, Ignore>,
    timeout_state: TimeoutState,
    timeout_delay: u64,
}

impl<'a, const BUFFER_SIZE: usize> RingBufferUsbWriter<'a, BUFFER_SIZE> {
    const DEFAULT_TIMEOUT_DELAY: u64 = SystemTimer::TICKS_PER_SECOND / 1_000; // 1ms


    pub fn new(usb: impl Peripheral<P = USB_DEVICE> + 'a, timeout_delay: Option<u64>) -> Self {
        Self::new_from_ref(usb.into_ref(), timeout_delay)
    }

    pub fn new_from_ref(usb: PeripheralRef<'a, USB_DEVICE>, timeout_delay: Option<u64>) -> Self {
        Self {
            usb,
            buffer: RingBuffer::new(),
            timeout_state: TimeoutState::None,
            timeout_delay: timeout_delay.unwrap_or(Self::DEFAULT_TIMEOUT_DELAY),
        }
    }

    pub fn enable_interrupt(&mut self) {
        interrupts::usb_interrupt_enable(Some(Priority::Priority9));
    }

    pub fn update(&mut self, qq: &mut impl QQAlarmQueue) -> bool {
        // currently only serial_in_empty interupt is possible
        let pending_interrupts = interrupts::usb_interrupt_get_and_clear(USBInterruptStatus::SERIAL_IN_EMPTY);

        if pending_interrupts.is_empty() {
            if let TimeoutState::Pending(timeout_start) = self.timeout_state {
                let qq_alarm_id = qq.add(timeout_start + self.timeout_delay).unwrap();
                self.timeout_state = TimeoutState::Active(qq_alarm_id);

                true
            } else {
                false
            }
        } else {
            while self.usb.ep1_conf().read().serial_in_ep_data_free().bit_is_set() {
                match self.buffer.pop_front() {
                    Some(byte) => {
                        self.usb.ep1().write(|w| unsafe { w.rdwr_byte().bits(byte) }); // TODO: safety
                    },
                    None => {
                        self.usb.ep1_conf().write(|w| w.wr_done().set_bit()); // flush
                        break;
                    }
                }
            }

            // TODO: cannot be None
            if let TimeoutState::Active(qq_alarm_id) = self.timeout_state {
                qq.remove(qq_alarm_id).unwrap();
            }

            if self.buffer.len() == 0 {
                self.usb.int_ena().modify(|_, w| w.serial_in_empty().clear_bit()); // disable interupt

                self.timeout_state = TimeoutState::None;
            } else {
                let qq_alarm_id = qq.add(SystemTimer::now()).unwrap();
                self.timeout_state = TimeoutState::Active(qq_alarm_id);
            }

            true
        }
    }

    pub fn on_alarm(&mut self, qq_alarm_id: usize) -> bool {
        if let TimeoutState::Active(id) = self.timeout_state && id == qq_alarm_id {
            self.timeout_state = TimeoutState::Timeout;

            true
        } else {
            false
        }
    }
}

impl<'a, const BUFFER_SIZE: usize> UsbWriter for RingBufferUsbWriter<'a, BUFFER_SIZE> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), RingBufferError> {
        let empty_before = self.buffer.len() == 0;

        self.buffer.extend_from_slice(bytes)?;

        if empty_before {
            // TODO: must be None or Timeout before
            if self.timeout_state == TimeoutState::None {
                self.timeout_state = TimeoutState::Pending(SystemTimer::now());
            }

            self.usb.int_ena().modify(|_, w| w.serial_in_empty().set_bit()); // enable interupt
        }

        Ok(())
    }

    fn is_timeouted(&self) -> bool {
        self.timeout_state == TimeoutState::Timeout
    }
}

impl<'a, const BUFFER_SIZE: usize> Write for RingBufferUsbWriter<'a, BUFFER_SIZE> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write(s.as_bytes()).map_err(|_| core::fmt::Error)
    }
}