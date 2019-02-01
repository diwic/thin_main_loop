use std::marker::PhantomData;
use crate::{CbKind, CbId, MainLoopError, IODirection};
use glib_sys;
use std::{mem, panic};
use crate::mainloop::{SendFnOnce, ffi_cb_wrapper};
use std::os::raw::c_uint;

const G_SOURCE_FUNCS: glib_sys::GSourceFuncs = glib_sys::GSourceFuncs {
    prepare: None,// Option<unsafe extern "C" fn(_: *mut GSource, _: *mut c_int) -> gboolean>,
    check: Some(glib_source_check_cb), // Option<unsafe extern "C" fn(_: *mut GSource) -> gboolean>,
    dispatch: Some(glib_source_dispatch_cb), // Option<unsafe extern "C" fn(_: *mut GSource, _: GSourceFunc, _: gpointer) -> gboolean>,
    finalize: None, // Option<unsafe extern "C" fn(_: *mut GSource)>,
    closure_callback: None, // GSourceFunc,
    closure_marshal: None, // GSourceDummyMarshal,
};

#[repr(C)]
struct GSourceData {
    gsource: glib_sys::GSource,
//    funcs: glib_sys::GSourceFuncs,
    tag: glib_sys::gpointer,
}

pub struct Backend<'a> {
    ctx: *mut glib_sys::GMainContext,
    _z: PhantomData<&'a u8>
}

unsafe extern "C" fn glib_source_check_cb(gs: *mut glib_sys::GSource) -> glib_sys::gboolean {
    ffi_cb_wrapper(|| {
        let tag = {
            let ss: &mut GSourceData = &mut *(gs as *mut _);
            ss.tag
        };
        let cond = glib_sys::g_source_query_unix_fd(gs, tag);
        // println!("Check {:?} {:?}!", tag, cond);
        if cond == 0 { glib_sys::GFALSE } else { glib_sys::GTRUE }
   }, glib_sys::GFALSE)
}

unsafe extern "C" fn glib_source_dispatch_cb(gs: *mut glib_sys::GSource, _: glib_sys::GSourceFunc, x: glib_sys::gpointer) -> glib_sys::gboolean {
    ffi_cb_wrapper(|| {
        // println!("Dispatch!");
        let x = x as *mut _ as *mut CbKind;
        let tag = {
            let ss: &mut GSourceData = &mut *(gs as *mut _);
            ss.tag
        };
        let cond = glib_sys::g_source_query_unix_fd(gs, tag);
        let dir = gio_to_dir(cond);

        let r = (*x).call_mut(Some(dir));
        if !r { (*Box::from_raw(x)).post_call_mut() };
        if r { glib_sys::GTRUE } else { glib_sys::GFALSE }
   }, glib_sys::GFALSE)
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
        let x = x as *mut _ as *mut CbKind;
        let r = (*x).call_mut(None);
        if !r { (*Box::from_raw(x)).post_call_mut() };
        if r { glib_sys::GTRUE } else { glib_sys::GFALSE }
   }, glib_sys::GFALSE)
}

struct Dummy(Box<dyn FnOnce() + Send + 'static>);

struct Sender(*mut glib_sys::GMainContext);

unsafe impl Send for Sender {}

impl Drop for Sender {
    fn drop(&mut self) { unsafe { glib_sys::g_main_context_unref(self.0) } }
}

unsafe extern fn glib_send_cb(x: glib_sys::gpointer) -> glib_sys::gboolean {
    ffi_cb_wrapper(|| {
        let x: Box<Dummy> = Box::from_raw(x as *mut _);
        let f = x.0;
        f();
    }, ());
    glib_sys::GFALSE
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
        r
    }
    pub (crate) fn push(&self, cb: CbKind<'a>) -> Result<CbId, MainLoopError> {
        let s = unsafe { match &cb {
            CbKind::IO(io) => {
                let s = glib_sys::g_source_new(&G_SOURCE_FUNCS as *const _ as *mut _, mem::size_of::<GSourceData>() as u32);
                let tag = glib_sys::g_source_add_unix_fd(s, io.fd(), dir_to_gio(io.direction()));
                // println!("Tag: {:?}", tag);
                let ss: &mut GSourceData = &mut *(s as *mut _);
                ss.tag = tag;
                s
            },
            CbKind::Asap(_) => glib_sys::g_idle_source_new(),
            CbKind::After(_,_) | CbKind::Interval(_,_) => 
                glib_sys::g_timeout_source_new(cb.duration_millis()?.unwrap()),
        }};

        let x = Box::into_raw(Box::new(cb));
        let x = x as *mut _ as glib_sys::gpointer;
        unsafe {
            glib_sys::g_source_set_callback(s, Some(glib_cb), x, None);
            glib_sys::g_source_set_priority(s, glib_sys::G_PRIORITY_DEFAULT);
            glib_sys::g_source_attach(s, self.ctx);
            glib_sys::g_source_unref(s);
        }
        Ok(CbId())
    }
}

