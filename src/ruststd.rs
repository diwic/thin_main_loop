use std::cell::RefCell;
use std::collections::VecDeque;
use crate::{CbKind, CbId, MainLoopError};
use std::time::{Instant, Duration};
use std::thread;
use crate::mainloop::SendFnOnce;
use std::sync::mpsc::{channel, Sender, Receiver};
use boxfnonce::SendBoxFnOnce;

struct Data<'a> {
    id: CbId,
    next: Instant,
    kind: CbKind<'a>,
}

struct TSender {
    thread: thread::Thread,
    sender: Sender<SendBoxFnOnce<'static, ()>>,
}

impl SendFnOnce for TSender {
    fn send(&self, f: SendBoxFnOnce<'static, ()>) -> Result<(), MainLoopError> {
        self.sender.send(f).map_err(|e| MainLoopError::Other(e.into()))?;
        self.thread.unpark();
        Ok(())
    }
}

pub struct Backend<'a> {
    data: RefCell<VecDeque<Data<'a>>>,
    recv: Receiver<SendBoxFnOnce<'static, ()>>,
}

impl<'a> Backend<'a> {
    pub (crate) fn new() -> Result<(Self, Box<SendFnOnce>), MainLoopError> {
        let (tx, rx) = channel();
        let be = Backend { recv: rx, data: Default::default() };
        let sender = TSender { thread: thread::current(), sender: tx };
        Ok((be, Box::new(sender)))
    }

    pub fn run_one(&self, wait: bool) -> bool {
        let mut d = self.data.borrow_mut();
        let mut item = d.pop_front();
        let mut next: Option<Instant> = item.as_ref().map(|i| i.next);
        let now = Instant::now();
        if let Some(n) = next {
            if n > now {
                d.push_front(item.take().unwrap());
            } else {
                next = d.get(0).map(|i| i.next);
            }
        }
        drop(d);

        if item.is_none() {
            if let Ok(cb) = self.recv.try_recv() {
                cb.call();
                return true;
            }
        }

        if let Some(mut item) = item {
            if item.kind.call_mut(None) {
                // Remain on the main loop
                item.next += item.kind.duration().unwrap();
                self.push_internal(item);
            } else { item.kind.post_call_mut() }
            true
        } else if wait {
            if let Some(next) = next {
                thread::park_timeout(next - now);
            } else {
                thread::park();
            }
            false
        } else { false }
    }

    fn push_internal(&self, item: Data<'a>) {
        let mut d = self.data.borrow_mut();
        let mut i = 0;
        while let Some(x) = d.get(i) {
            if x.next > item.next { break; } else { i += 1; }
        }
        d.insert(i, item);
    }

    pub (crate) fn push(&self, id: CbId, cb: CbKind<'a>) -> Result<(), MainLoopError> {
        if let CbKind::IO(_) = &cb { return Err(MainLoopError::Unsupported) };
        self.push_internal(Data {
            id: id,
            next: Instant::now() + cb.duration().unwrap_or(Duration::from_secs(0)),
            kind: cb
        });
        Ok(())
    }

    pub (crate) fn cancel(&self, id: CbId) -> Option<CbKind<'a>> {
        let mut d = self.data.borrow_mut();
        d.iter().position(|x| x.id == id)
            .and_then(|idx| d.remove(idx))
            .map(|data| data.kind)
    }
}
