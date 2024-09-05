use core::{cmp, iter};


use esp_hal::{
    interrupt::Priority, prelude::*, timer::systimer::{Alarm, SystemTimer, Target}, Blocking
};

use crate::interrupts::{self, SystimerTartet0InterruptStatus};



#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QQAlarmError {
    QueueFull,
    IdNotFound,
}


pub trait QQAlarmQueue {
    fn add(&mut self, wake_at: u64) -> Result<usize, QQAlarmError>;
    // fn debug_add(&mut self, wake_at: u64, uw: &mut impl Write) -> Result<usize, QQAlarmError>;
    fn remove(&mut self, id: usize) -> Result<(), QQAlarmError>;
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QQAlarmState { Waiting, Pending }

#[derive(Debug, Clone, Copy)]
struct QQAlarm {
    id: usize,
    wake_at: u64,
    state: QQAlarmState,
}


/// simple QQ alarm queue, no algorithms are used for optimaztion (e.g.: priority queues, ...)
// #[derive(Debug)]
pub struct DumbQQAlarmQueue<const N: usize> {
    alarm: Alarm<Target, Blocking, 0>,
    queue: [Option<QQAlarm>; N],
    next_wakeup: Option<u64>,
    next_id: usize,
    any_pending: bool,
}

impl<const N: usize> DumbQQAlarmQueue<N> {
    pub fn new(alarm: Alarm<Target, Blocking, 0>) -> Self {
        DumbQQAlarmQueue {
            alarm,
            queue: [None; N],
            next_wakeup: None,
            next_id: 0,
            any_pending: false,
        }
    }

    pub fn enable_interrupt(&mut self) {
        interrupts::systimer_target0_interrupt_enable(Some(Priority::Priority10));
    }

    pub fn update(&mut self) -> bool {
        // only target interrupt is possible
        let qq_alarm_pending = interrupts::systimer_target0_interrupt_get_and_clear(SystimerTartet0InterruptStatus::TARGET);

        if qq_alarm_pending.is_empty() {
            return false;
        }

        let now = SystemTimer::now();

        let mut min_wake_at = None;

        for qq_alarm in self.queue.iter_mut().filter_map(|qq_alarm| qq_alarm.as_mut()).filter(|qq_alarm| qq_alarm.state == QQAlarmState::Waiting) {
            let wake_at = qq_alarm.wake_at;

            if wake_at <= now {
                self.any_pending = true;
                qq_alarm.state = QQAlarmState::Pending;
            } else {
                min_wake_at = Some(min_wake_at.map_or(wake_at, |min_wake_at| cmp::min(min_wake_at, wake_at)));
            }
        }

        self.next_wakeup = min_wake_at;

        if let Some(min_wake_at) = min_wake_at {
             // TODO: in documentation is written that you can set target walue lower then `now`, but it doesn't seem to be working here
             //       (it worked in separate test)
            let now = SystemTimer::now();
            self.alarm.set_target(cmp::max(now + 250, min_wake_at));
        } else {
            self.alarm.enable_interrupt(false);
        }

        true
    }

    /// returned iterator should be fully consumed to free up space in queue
    /// e.g. `queue.consume_pending().unwrap().take(3)` will cause problems, because fourth pending alarm in iterator will never be consumed and therefore not freed
    /// if you do not consume whole iterator at one time, be sure to call `consume_pending` again
    pub fn consume_pending<'a>(&'a mut self) -> Option<impl Iterator<Item = usize> + 'a> {
        if !self.any_pending {
            return None;
        }

        Some(self.queue.iter_mut()
            .map(|qq_alarm_opt| {
                if let Some(qq_alarm) = qq_alarm_opt && qq_alarm.state == QQAlarmState::Pending {
                    let id = qq_alarm.id;
                    *qq_alarm_opt = None;
                    Some(id)
                } else {
                    None
                }
            })
            .chain(iter::once_with(|| {
                // sets `any_pending` to false after all pending alarms are set to `None`
                self.any_pending = false;
                None
            }))
            .filter_map(|id| id)
        )
    }
}

impl<const N: usize> QQAlarmQueue for DumbQQAlarmQueue<N> {
    fn add(&mut self, wake_at: u64) -> Result<usize, QQAlarmError> {
        // assuming wake_at is less than now (if it is not it is ok alarm will cause interrupt instantly)
        let id = self.next_id;
        self.next_id += 1;
    
        let empty_alarm = self.queue.iter_mut().find(|alarm| alarm.is_none()).ok_or(QQAlarmError::QueueFull)?;
        *empty_alarm = Some(QQAlarm {
            id,
            wake_at,
            state: QQAlarmState::Waiting,
        });
    
        let set_target = match self.next_wakeup {
            Some(next_wakeup) => wake_at < next_wakeup,
            None => {
                self.alarm.clear_interrupt();
                self.alarm.enable_interrupt(true);
                true
            }
        };
    
        if set_target {
            self.alarm.set_target(wake_at);
            self.next_wakeup = Some(wake_at);
        }
    
        Ok(id)
    }

    fn remove(&mut self, id: usize) -> Result<(), QQAlarmError> {
        let mut id_found = false;
    
        let mut min_wake_at = None;
        let mut any_pending = false;

        for qq_alarm_opt in self.queue.iter_mut() {
            if let Some(qq_alarm) = qq_alarm_opt {
                if qq_alarm.id == id {
                    id_found = true;
                    *qq_alarm_opt = None
                } else {
                    match qq_alarm.state {
                        QQAlarmState::Waiting => {
                            let wake_at = qq_alarm.wake_at;
                            min_wake_at = Some(min_wake_at.map_or(wake_at, |min_wake_at| cmp::min(min_wake_at, wake_at)));
                        },
                        QQAlarmState::Pending => {
                            any_pending = true;
                        }
                    }
                }
            }
        }

        if !id_found {
            return Err(QQAlarmError::IdNotFound);
        }

        match min_wake_at {
            None => {
                if self.next_wakeup.is_some() {
                    self.alarm.enable_interrupt(false);
                    self.next_wakeup = None;
                }
            },
            Some(min_wake_at) => {
                // `alarm_queue.next_wakeup` cannot be `None` because we found some waiting alarms
                if min_wake_at != self.next_wakeup.unwrap() {
                    self.alarm.set_target(min_wake_at);
                    self.next_wakeup = Some(min_wake_at);
                }
            },
        }

        // update to `any_pending` is needed when deleted alarm was pending alarm and all other alarms were not pending (`any_pending` is changed from `true` to `false`)
        self.any_pending = any_pending;

        Ok(())
    }
}