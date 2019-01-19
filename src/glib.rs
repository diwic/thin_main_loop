use std::marker::PhantomData;
use crate::{CbKind, CbId, MainLoopError};
use glib_sys;
use std::panic;
use std::any::Any;
use std::cell::RefCell;
use crate::mainloop::SendFnOnce;

pub struct Backend<'a> {
    ctx: *mut glib_sys::GMainContext,
    _z: PhantomData<&'a u8>
}

unsafe extern fn glib_cb(x: glib_sys::gpointer) -> glib_sys::gboolean {
    match panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let x = x as *mut _ as *mut CbKind;
        if let CbKind::Interval(f, _) = &mut (*x) { 
            if f() { return true }
        }
        match *Box::from_raw(x) {
            CbKind::After(f, _) => f(),
            CbKind::Asap(f) => f(),
            CbKind::Interval(_, _) => {},
        }
        false
   }))
   {
        Ok(x) => if x { glib_sys::GTRUE } else { glib_sys::GFALSE }
        Err(e) => {
            current_panic.with(|cp| { *cp.borrow_mut() = Some(e) });
            glib_sys::GFALSE
        }
   }
}

struct Dummy(Box<dyn FnOnce() + Send + 'static>);

struct Sender(*mut glib_sys::GMainContext);

unsafe impl Send for Sender {}

impl Drop for Sender {
    fn drop(&mut self) { unsafe { glib_sys::g_main_context_unref(self.0) } }
}

unsafe extern fn glib_send_cb(x: glib_sys::gpointer) -> glib_sys::gboolean {
    match panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let x: Box<Dummy> = Box::from_raw(x as *mut _);
        let f = x.0;
        f()
   }))
   {
        Err(e) => {
            current_panic.with(|cp| { *cp.borrow_mut() = Some(e) });
        },
        _ => {},
    }    
    glib_sys::GFALSE
}


thread_local! {
    static current_panic: RefCell<Option<Box<dyn Any + Send + 'static>>> = Default::default();
}


impl SendFnOnce for Sender {
    fn send(&self, f: Box<FnOnce() + Send + 'static>) -> Result<(), MainLoopError> {
        let f = Box::new(Dummy(f));
        let f = Box::into_raw(f);
        let f = f as *mut _ as glib_sys::gpointer;
        // FIXME: glib docs are a bit vague on to what degree a GMainContext is equivalent to a thread.
        // Nonetheless this seems to be the recommended way to do things. But we should probably put
        // safeguards here or in glib_send_cb
        unsafe { glib_sys::g_main_context_invoke(self.0, Some(glib_send_cb), f); }
        Ok(())
    }
}

impl Drop for Backend<'_> {
    fn drop(&mut self) { unsafe { glib_sys::g_main_context_unref(self.ctx) } }
}

impl<'a> Backend<'a> {
    pub (crate) fn new() -> Result<(Self, Box<SendFnOnce>), MainLoopError> { 
        let be = Backend {
            ctx: unsafe { glib_sys::g_main_context_new() }, 
            _z: PhantomData 
        };
        let sender = Sender(unsafe { glib_sys::g_main_context_ref(be.ctx) }); 
        Ok((be, Box::new(sender)))
    }
    pub fn run_one(&self, wait: bool) -> bool {
        let w = if wait { glib_sys::GTRUE } else { glib_sys::GFALSE };
        let r = unsafe { glib_sys::g_main_context_iteration(self.ctx, w) != glib_sys::GFALSE };
        if let Some(e) = current_panic.with(|cp| { cp.borrow_mut().take() }) { panic::resume_unwind(e) }
        r
    }
    pub (crate) fn push(&self, cb: CbKind<'a>) -> Result<CbId, MainLoopError> {
        let d = cb.duration_millis()?;
        let x = Box::into_raw(Box::new(cb));
        let x = x as *mut _ as glib_sys::gpointer;
        unsafe { 
            let s = if let Some(d) = d {
                glib_sys::g_timeout_source_new(d)
            } else {
                glib_sys::g_idle_source_new()
            };
            glib_sys::g_source_set_callback(s, Some(glib_cb), x, None);
            glib_sys::g_source_set_priority(s, glib_sys::G_PRIORITY_DEFAULT);
            glib_sys::g_source_attach(s, self.ctx);
            glib_sys::g_source_unref(s);
        }
        Ok(CbId())
    }
}

