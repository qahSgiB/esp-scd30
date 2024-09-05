use esp_hal::{peripheral::PeripheralRef, peripherals::I2C0, timer::systimer::SystemTimer};

use crate::{
    interrupts::{self, I2CInterruptStatus},
    machines::Delay,
    qq_alarm_queue::QQAlarmQueue,
    sdc::{self, SDCGetCommand, SDCSetCommand},
    pac_utils::i2c::I2CTransmissionError
};



#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State<T> {
    Active(bool),
    Done(T)
}



#[derive(Debug, Clone, Copy)]
pub enum SetState {
    AwaitingInterrupt,
    Done,
}

#[derive(Debug)]
pub struct Set {
    state: SetState,
}

impl Set {
    pub fn start(i2c: PeripheralRef<I2C0>, command: SDCSetCommand) -> Set {
        sdc::set_command_write(i2c, command);
        Set {
            state: SetState::AwaitingInterrupt,
        }
    }

    pub fn update(&mut self) -> State<Result<(), I2CTransmissionError>> {
        match self.state {
            SetState::AwaitingInterrupt => {
                let pending_interrupts = interrupts::i2c_interrupt_get_and_clear(I2CInterruptStatus::all());
    
                if pending_interrupts.is_empty() {
                    State::Active(false)
                } else {
                    self.state = SetState::Done;
                    let maybe_err = I2CTransmissionError::from_interrupt_flags(pending_interrupts);
                    State::Done(if let Some(err) = maybe_err { Err(err) } else { Ok(()) })
                }
            },
            SetState::Done => State::Done(Ok(())),
        }
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelayedGetError {
    Write(I2CTransmissionError),
    Read(I2CTransmissionError),
}

#[derive(Debug, Clone, Copy)]
enum DelayedGetState {
    WriteAwaitingInterrupt,
    Delay(Delay),
    ReadAwaitingInterrupt,
    Done,
}

#[derive(Debug)]
pub struct DelayedGet {
    state: DelayedGetState,
    command: SDCGetCommand,
    delta: u64, // TODO: unit
}

impl DelayedGet {
    pub fn start(i2c: PeripheralRef<I2C0>, command: SDCGetCommand, delta: u64) -> DelayedGet {
        sdc::get_command_write(i2c, command);

        DelayedGet {
            state: DelayedGetState::WriteAwaitingInterrupt,
            command,
            delta,
        }
    }

    pub fn update(&mut self, qq: &mut impl QQAlarmQueue, i2c: PeripheralRef<I2C0>) -> State<Result<(), DelayedGetError>> {
        match self.state {
            DelayedGetState::WriteAwaitingInterrupt => {
                let pending_interrupts = interrupts::i2c_interrupt_get_and_clear(I2CInterruptStatus::all());
    
                if pending_interrupts.is_empty() {
                    State::Active(false)
                } else {
                    if let Some(err) = I2CTransmissionError::from_interrupt_flags(pending_interrupts) {
                        self.state = DelayedGetState::Done;
                        State::Done(Err(DelayedGetError::Write(err)))
                    } else {
                        let wake_at = SystemTimer::now() + self.delta;
                        let qq_alarm_id = qq.add(wake_at).unwrap();
                        self.state = DelayedGetState::Delay(Delay::new(qq_alarm_id));

                        State::Active(true)
                    }
                }
            },
            DelayedGetState::Delay(Delay::Done) => {
                sdc::get_command_read(i2c, self.command);
                self.state = DelayedGetState::ReadAwaitingInterrupt;

                State::Active(true)
            },
            DelayedGetState::ReadAwaitingInterrupt => {
                let pending_interrupts = interrupts::i2c_interrupt_get_and_clear(I2CInterruptStatus::all());
    
                if pending_interrupts.is_empty() {
                    State::Active(false)
                } else {
                    self.state = DelayedGetState::Done;
                    let maybe_err = I2CTransmissionError::from_interrupt_flags(pending_interrupts);
                    State::Done(if let Some(err) = maybe_err { Err(DelayedGetError::Read(err)) } else { Ok(()) })
                }
            },
            DelayedGetState::Done => State::Done(Ok(())),
            DelayedGetState::Delay(Delay::Waiting { .. }) => State::Active(false),
        }
    }

    pub fn on_alarm(&mut self, qq_alarm_id: usize) -> bool {
        match &mut self.state {
            DelayedGetState::Delay(delay) => delay.on_alarm(qq_alarm_id),
            _ => false,
        }
    }
}
