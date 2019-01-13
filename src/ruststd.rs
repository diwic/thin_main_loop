use std::cell::RefCell;
use std::collections::VecDeque;
use crate::{MainLoop};
use std::time::{Instant, Duration};
use std::thread;

/*
struct After<'a> {
    id: CbId,
    time: Instant,
    cb: Box<dyn FnOnce(&MainLoop<'a>) + 'a>
}

struct Intervals<'a> {
    id: CbId,
    period: Duration,
    next: Instant,
    cb: Box<dyn FnMut(&MainLoop<'a>) -> bool + 'a>
}
*/

struct Data<'a> {
//    id: CbId,
    next: Instant,
    kind: Kind<'a>,
}

enum Kind<'a> { 
    After(Box<dyn FnOnce(&MainLoop<'a>) + 'a>),
    Interval(Box<dyn FnMut(&MainLoop<'a>) -> bool + 'a>, Duration)
}

#[derive(Default)]
pub struct Backend<'a> {
    data: RefCell<VecDeque<Data<'a>>>,
}

impl<'a> Backend<'a> {
    pub fn new() -> Self { Default::default() }
    pub fn run_one(&self, ml: &MainLoop<'a>, wait: bool) -> bool {
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

        if let Some(item) = item {
            match item.kind {
                Kind::After(f) => f(ml),
                Kind::Interval(mut f, d) => if f(ml) {
                    self.push(Data { /* id: item.id, */ next: item.next + d, kind: Kind::Interval(f, d)});
                },
            }
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
    fn push(&self, item: Data<'a>) {
        let mut d = self.data.borrow_mut();
        let mut i = 0;
        while let Some(x) = d.get(i) {
            if x.next > item.next { break; } else { i += 1; }
        }
        d.insert(i, item);
    }
    pub fn push_after(&self, d: Duration, f: Box<dyn FnOnce(&MainLoop<'a>) + 'a>) {
        self.push(Data { next: Instant::now() + d, kind: Kind::After(f) });
    }
    pub fn push_interval(&self, d: Duration, f: Box<dyn FnMut(&MainLoop<'a>) -> bool + 'a>) {
        self.push(Data { next: Instant::now() + d, kind: Kind::Interval(f, d) });
    }
}
