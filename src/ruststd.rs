use std::cell::RefCell;
use std::collections::VecDeque;
use crate::CbKind;
use std::time::{Instant, Duration};
use std::thread;

struct Data<'a> {
//    id: CbId,
    next: Instant,
    kind: CbKind<'a>,
}

#[derive(Default)]
pub struct Backend<'a> {
    data: RefCell<VecDeque<Data<'a>>>,
}

impl<'a> Backend<'a> {
    pub fn new() -> Self { Default::default() }
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

        if let Some(item) = item {
            match item.kind {
                CbKind::Asap(f) => f(),
                CbKind::After(f, _) => f(),
                CbKind::Interval(mut f, d) => if f() {
                    self.push_internal(Data { /* id: item.id, */ next: item.next + d, kind: CbKind::Interval(f, d)});
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
    fn push_internal(&self, item: Data<'a>) {
        let mut d = self.data.borrow_mut();
        let mut i = 0;
        while let Some(x) = d.get(i) {
            if x.next > item.next { break; } else { i += 1; }
        }
        d.insert(i, item);
    }
    pub (crate) fn push(&self, cb: CbKind<'a>) {
        self.push_internal(Data {
            next: Instant::now() + cb.duration().unwrap_or(Duration::from_secs(0)),
            kind: cb
        })
    }
}
