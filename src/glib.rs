use crate::{CbKind, CbId, MainLoopError, IODirection};
use glib_sys;
use std::{mem, panic};
use std::ptr::NonNull;
use crate::mainloop::{SendFnOnce, ffi_cb_wrapper};
use std::os::raw::c_uint;
use boxfnonce::SendBoxFnOnce;

use std::cell::RefCell;
use std::collections::{HashMap};

const G_SOURCE_FUNCS: glib_sys::GSourceFuncs = glib_sys::GSourceFuncs {
    prepare: None,// Option<unsafe extern "C" fn(_: *mut GSource, _: *mut c_int) -> gboolean>,
    check: Some(glib_source_check_cb), // Option<unsafe extern "C" fn(_: *mut GSource) -> gboolean>,
    dispatch: Some(glib_source_dispatch_cb), // Option<unsafe extern "C" fn(_: *mut GSource, _: GSourceFunc, _: gpointer) -> gboolean>,
    finalize: Some(glib_source_finalize_cb), // <unsafe extern "C" fn(_: *mut GSource)>,
    closure_callback: None, // GSourceFunc,
    closure_marshal: None, // GSourceDummyMarshal,
};

#[repr(C)]
struct GSourceIOData {
    gsource: glib_sys::GSource,
    tag: glib_sys::gpointer,
    cb_data: Option<NonNull<CbData<'static>>>,
}

struct CbData<'a> {
    gsource_ptr: *mut glib_sys::GSource,
    cbid: CbId,
    kind: RefCell<Option<CbKind<'a>>>,
}

thread_local! {
    static finished_tls: RefCell<Vec<CbId>> = Default::default();
}

pub struct Backend<'a> {
    ctx: *mut glib_sys::GMainContext,
    cb_map: RefCell<HashMap<CbId, Box<CbData<'a>>>>,
}

unsafe extern "C" fn glib_source_finalize_cb(gs: *mut glib_sys::GSource) {
    ffi_cb_wrapper(|| {
        let ss: &mut GSourceIOData = &mut *(gs as *mut _);
        if let Some(cb_data) = &ss.cb_data {
            finished_tls.with(|f| { f.borrow_mut().push(cb_data.as_ref().cbid); });
            ss.cb_data.take();
        }
    }, ())
}

unsafe extern "C" fn glib_source_check_cb(gs: *mut glib_sys::GSource) -> glib_sys::gboolean {
    ffi_cb_wrapper(|| {
        let tag = {
            let ss: &mut GSourceIOData = &mut *(gs as *mut _);
            ss.tag
        };
        let cond = glib_sys::g_source_query_unix_fd(gs, tag);
        // println!("Check {:?} {:?}!", tag, cond);
        if cond == 0 { glib_sys::GFALSE } else { glib_sys::GTRUE }
   }, glib_sys::GFALSE)
}

unsafe extern "C" fn glib_source_dispatch_cb(gs: *mut glib_sys::GSource, _: glib_sys::GSourceFunc, _: glib_sys::gpointer) -> glib_sys::gboolean {
    ffi_cb_wrapper(|| {
        let ss: &mut GSourceIOData = &mut *(gs as *mut _);
        let cond = glib_sys::g_source_query_unix_fd(gs, ss.tag);
        let dir = gio_to_dir(cond);

        let r = false;
        if let Some(mut cb_data) = &ss.cb_data {
            if cbdata_call(cb_data.as_mut(), Some(dir)) { return glib_sys::GTRUE; }
        }
        ss.cb_data.take();
        glib_sys::GFALSE
   }, glib_sys::GFALSE)
}

fn cbdata_call(cb_data: &CbData, dir: Option<Result<IODirection, std::io::Error>>) -> bool {
    if let Some(ref mut kind) = *cb_data.kind.borrow_mut() {
        if kind.call_mut(dir) { return true; }
    };
    cb_data.kind.borrow_mut().take().map(|kind| { kind.post_call_mut(); });
    finished_tls.with(|f| { f.borrow_mut().push(cb_data.cbid); });
    false
}

fn dir_to_gio(d: IODirection) -> glib_sys::GIOCondition {
    glib_sys::G_IO_HUP + glib_sys::G_IO_ERR + match d {
        IODirection::None => 0,
        IODirection::Read => glib_sys::G_IO_IN,
        IODirection::Write => glib_sys::G_IO_OUT,
        IODirection::Both => glib_sys::G_IO_IN + glib_sys::G_IO_OUT,
    }
}

fn gio_to_dir(cond: glib_sys::GIOCondition) -> Result<IODirection, std::io::Error> {
    const BOTH: c_uint = glib_sys::G_IO_IN + glib_sys::G_IO_OUT;
    match cond {
       0 => Ok(IODirection::None),
       glib_sys::G_IO_IN => Ok(IODirection::Read),
       glib_sys::G_IO_OUT => Ok(IODirection::Write),
       BOTH => Ok(IODirection::Both),
       _ => unimplemented!(),
    }
}

unsafe extern fn glib_cb(x: glib_sys::gpointer) -> glib_sys::gboolean {
    ffi_cb_wrapper(|| {
        let x = x as *const _ as *mut CbData;
        // println!("cb: {:?}", x);
        // println!("cbid: {:?}", (*x).cbid);
        if cbdata_call(&mut (*x), None) { glib_sys::GTRUE } else { glib_sys::GFALSE }
   }, glib_sys::GFALSE)
}

struct Dummy(SendBoxFnOnce<'static, ()>);

struct Sender(*mut glib_sys::GMainContext);

unsafe impl Send for Sender {}

impl Drop for Sender {
    fn drop(&mut self) { unsafe { glib_sys::g_main_context_unref(self.0) } }
}

unsafe extern fn glib_send_cb(x: glib_sys::gpointer) -> glib_sys::gboolean {
    ffi_cb_wrapper(|| {
        let x: Box<Dummy> = Box::from_raw(x as *mut _);
        let f = x.0;
        f.call();
    }, ());
    glib_sys::GFALSE
}

impl SendFnOnce for Sender {
    fn send(&self, f: SendBoxFnOnce<'static, ()>) -> Result<(), MainLoopError> {
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
    fn drop(&mut self) {
        finished_tls.with(|f| { f.borrow_mut().clear(); }); 
        // self.cb_map().map(|cbid| { self.cancel(cbid) }).count();
        unsafe { glib_sys::g_main_context_unref(self.ctx) }
    }
}

impl<'a> Backend<'a> {
    pub (crate) fn new() -> Result<(Self, Box<SendFnOnce>), MainLoopError> { 
        let be = Backend {
            ctx: unsafe { glib_sys::g_main_context_new() }, 
            cb_map: Default::default(),
        };
        finished_tls.with(|stls| {
            *stls.borrow_mut() = Default::default();
        });
        let sender = Sender(unsafe { glib_sys::g_main_context_ref(be.ctx) }); 
        Ok((be, Box::new(sender)))
    }

    pub fn run_one(&self, wait: bool) -> bool {
        let w = if wait { glib_sys::GTRUE } else { glib_sys::GFALSE };
        let r = unsafe { glib_sys::g_main_context_iteration(self.ctx, w) != glib_sys::GFALSE };
        finished_tls.with(|f| {
            for cbid in f.borrow_mut().drain(..) {
                self.cb_map.borrow_mut().remove(&cbid); // gsource has already been destroyed
            };
        });
        r
    }

    pub (crate) fn cancel(&self, cbid: CbId) -> Option<CbKind<'a>> {
        self.cb_map.borrow_mut().remove(&cbid)
        .and_then(|s| {
            unsafe {
                glib_sys::g_source_destroy(s.gsource_ptr);
                s.kind.borrow_mut().take()
            }
        })
    }

    pub (crate) fn push(&self, cbid: CbId, cb: CbKind<'a>) -> Result<(), MainLoopError> {
        let mut tag = None;
        let s = unsafe { match &cb {
            CbKind::IO(io) => {
                let s = glib_sys::g_source_new(&G_SOURCE_FUNCS as *const _ as *mut _, mem::size_of::<GSourceIOData>() as u32);
                tag = Some(glib_sys::g_source_add_unix_fd(s, io.fd(), dir_to_gio(io.direction())));
                s
            },
            CbKind::Asap(_) => glib_sys::g_idle_source_new(),
            CbKind::After(_,_) | CbKind::Interval(_,_) => 
                glib_sys::g_timeout_source_new(cb.duration_millis()?.unwrap()),
        }};

        let boxed = Box::new(CbData {
            gsource_ptr: s,
            cbid: cbid,
            kind: RefCell::new(Some(cb)),
        });
        let x = NonNull::from(&*boxed);
        self.cb_map.borrow_mut().insert(cbid, boxed);

        unsafe {
            if let Some(tag) = tag {
                let ss: &mut GSourceIOData = &mut *(s as *mut _);
                ss.cb_data = Some(x.cast());
                ss.tag = tag;
            } else {
                // let x: *mut Rc<CbData> = &mut rc;
                // println!("push: {:?}, {:?}", x, rc.cbid);
                // println!("cbid: {:?}", (*x).cbid);
         
                glib_sys::g_source_set_callback(s, Some(glib_cb), x.as_ptr() as *mut _ as *mut _, None);
                // mem::forget(rc);
            }

            glib_sys::g_source_set_priority(s, glib_sys::G_PRIORITY_DEFAULT);
            glib_sys::g_source_attach(s, self.ctx);
            glib_sys::g_source_unref(s);
        }
        Ok(())
    }
}

