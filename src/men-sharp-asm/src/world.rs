//! Several programs wired together, the way behaviours are in a scene.
//!
//! Udon lets one behaviour do exactly three things to another: raise an
//! event on it by name, read one of its exported variables, and write one.
//! All three are synchronous, and the program they reach runs to its own
//! halt before the caller carries on. A [`World`] is that, and no more: a
//! list of programs, each with its own heap, and the routing between them.
//!
//! A program is taken out of the world while it runs, so the one that
//! called it cannot be reached at the same time. That is not a limitation
//! of the model but the thing the compiler's re-entry guard exists to
//! catch — here it surfaces as a loud error instead of a corrupted heap.

use std::rc::Rc;

use crate::emulator::{Due, Emulator, EmulatorError, Peers, Value};
use crate::program::Assembled;

/// One behaviour in the world.
struct Slot {
    name: String,
    assembled: Rc<Assembled>,
    /// Taken out while the program is running.
    emulator: Option<Emulator>,
}

pub struct World {
    slots: Vec<Slot>,
    /// Scene time and frame, shared by every program.
    pub time: f32,
    pub frame: i32,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    pub fn new() -> Self {
        World {
            slots: Vec::new(),
            time: 0.0,
            frame: 0,
        }
    }

    /// Adds a program under a name, and hands back the index that is its
    /// behaviour reference.
    pub fn add(&mut self, name: &str, assembled: Assembled, mut emulator: Emulator) -> usize {
        let index = self.slots.len();
        emulator.adopt_as(index);
        self.slots.push(Slot {
            name: name.to_string(),
            assembled: Rc::new(assembled),
            emulator: Some(emulator),
        });
        index
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.slots.iter().position(|slot| slot.name == name)
    }

    /// The program at `index`, for reading its heap between events.
    pub fn program(&self, index: usize) -> &Emulator {
        self.slots[index]
            .emulator
            .as_ref()
            .expect("the program is not running")
    }

    pub fn program_mut(&mut self, index: usize) -> &mut Emulator {
        self.slots[index]
            .emulator
            .as_mut()
            .expect("the program is not running")
    }

    /// What one program has logged. Each keeps its own list, so there is
    /// no single order across the world.
    pub fn log_of(&self, index: usize) -> &[String] {
        &self.program(index).log
    }

    /// Raises an event on a program from outside, as the runtime does.
    pub fn raise(&mut self, index: usize, event: &str) -> Result<(), EmulatorError> {
        self.send_event(index, event)
    }

    /// Moves the clock on and delivers every delayed event that falls due,
    /// earliest first, whichever program asked for it. An event may delay
    /// more; those are delivered too when they are due by then.
    pub fn advance(&mut self, seconds: f32, frames: i32) -> Result<(), EmulatorError> {
        self.time += seconds;
        self.frame += frames;
        for slot in &mut self.slots {
            if let Some(emulator) = &mut slot.emulator {
                emulator.time = self.time;
                emulator.frame = self.frame;
            }
        }
        loop {
            let Some((owner, position)) = self.next_due() else {
                return Ok(());
            };
            let event = self.slots[owner]
                .emulator
                .as_mut()
                .expect("an idle program owns the queue")
                .delayed
                .remove(position);
            let target = event.target.unwrap_or(owner);
            self.send_event(target, &event.event)?;
        }
    }

    /// The earliest delayed event that is due, as (program, position).
    fn next_due(&self) -> Option<(usize, usize)> {
        let time = self.time;
        let frame = self.frame;
        let mut best: Option<(usize, usize, f32)> = None;
        for (index, slot) in self.slots.iter().enumerate() {
            let Some(emulator) = &slot.emulator else {
                continue;
            };
            for (position, event) in emulator.delayed.iter().enumerate() {
                let ready = match event.due {
                    Due::Time(at) => at <= time,
                    // a frame's worth of events all fall due together; order
                    // them after anything with an earlier time
                    Due::Frame(at) => at <= frame,
                };
                if !ready {
                    continue;
                }
                let at = match event.due {
                    Due::Time(at) => at,
                    Due::Frame(_) => f32::NEG_INFINITY,
                };
                if best.is_none_or(|(_, _, best_at)| at < best_at) {
                    best = Some((index, position, at));
                }
            }
        }
        best.map(|(index, position, _)| (index, position))
    }

    pub fn has_pending_events(&self) -> bool {
        self.slots.iter().any(|slot| {
            slot.emulator
                .as_ref()
                .is_some_and(|emulator| !emulator.delayed.is_empty())
        })
    }

    fn take(
        &mut self,
        target: usize,
        what: &str,
    ) -> Result<(Emulator, Rc<Assembled>), EmulatorError> {
        let Some(slot) = self.slots.get_mut(target) else {
            return Err(EmulatorError::NoSuchProgram {
                target,
                what: what.to_string(),
            });
        };
        let assembled = slot.assembled.clone();
        match slot.emulator.take() {
            Some(emulator) => Ok((emulator, assembled)),
            None => Err(EmulatorError::NoSuchProgram {
                target,
                what: format!("{what}: the program is already running — it was re-entered"),
            }),
        }
    }
}

impl Peers for World {
    fn send_event(&mut self, target: usize, event: &str) -> Result<(), EmulatorError> {
        let (mut emulator, assembled) = self.take(target, event)?;
        // an event the program does not export is simply not raised, which
        // is what `SendCustomEvent` with an unknown name does on Udon
        let result = if assembled.entry_addresses.contains_key(event) {
            emulator.run_with(&assembled, event, self)
        } else {
            Ok(())
        };
        self.slots[target].emulator = Some(emulator);
        result
    }

    fn get_variable(&mut self, target: usize, name: &str) -> Result<Value, EmulatorError> {
        let (emulator, _) = self.take(target, name)?;
        // a variable the program does not export reads as null, as on Udon
        let value = emulator.value_of(name).cloned().unwrap_or_default();
        self.slots[target].emulator = Some(emulator);
        Ok(value)
    }

    fn set_variable(
        &mut self,
        target: usize,
        name: &str,
        value: Value,
    ) -> Result<(), EmulatorError> {
        let (mut emulator, _) = self.take(target, name)?;
        // ... and writing one it does not export does nothing
        emulator.set_value(name, value);
        self.slots[target].emulator = Some(emulator);
        Ok(())
    }
}
