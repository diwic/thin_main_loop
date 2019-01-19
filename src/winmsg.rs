use std::marker::PhantomData;
use crate::{CbKind, CbId, MainLoopError};
use crate::mainloop::SendFnOnce;
use winapi;
use std::{mem, ptr};
use std::sync::{Once, Arc};

use winapi::shared::windef::HWND;
use winapi::um::winuser;
use winapi::um::libloaderapi;
use winapi::um::winnt;

struct OwnedHwnd(HWND);

unsafe impl Send for OwnedHwnd {}
unsafe impl Sync for OwnedHwnd {}

impl Drop for OwnedHwnd {
    fn drop(&mut self) { unsafe { winuser::DestroyWindow(self.0); } }
}

pub struct Backend<'a> {
    wnd: Arc<OwnedHwnd>,
    _z: PhantomData<&'a u8>
}

impl SendFnOnce for Arc<OwnedHwnd> {
    fn send(&self, f: Box<FnOnce() + Send + 'static>) -> Result<(), MainLoopError> {
        let cb = CbKind::Asap(f);
        let x = Box::into_raw(Box::new(cb));
        unsafe {
            winuser::PostMessageA(self.0, WM_CALL_ASAP, x as usize, 0);
        }
        Ok(())
    }
}

const WM_CALL_ASAP: u32 = winuser::WM_USER + 10;
static WINDOW_CLASS: Once = Once::new();
static WINDOW_CLASS_NAME: &[u8] = b"Rust function dispatch\0";

fn ensure_window_class() {
    // println!("ensure_window_class window class start");
    WINDOW_CLASS.call_once(|| unsafe {
        // println!("Create window class start");
        let mut wc: winuser::WNDCLASSA = mem::zeroed();
        wc.lpszClassName = WINDOW_CLASS_NAME.as_ptr() as *const winnt::CHAR;
        wc.hInstance = libloaderapi::GetModuleHandleA(ptr::null_mut());
        wc.lpfnWndProc = Some(wnd_callback);
        winuser::RegisterClassA(&wc);
        // println!("Create window class finish");
    });
    // println!("ensure_window_class window class finish");
}

unsafe extern "system" fn wnd_callback(wnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    if msg == WM_CALL_ASAP || msg == winuser::WM_TIMER {
        let x = wparam as *mut CbKind;
        if let CbKind::Interval(f, _) = &mut (*x) { 
            if f() { return 0 };
        }
        match *Box::from_raw(x) {
            CbKind::After(f, _) => {
                winuser::KillTimer(wnd, wparam);
                f()
            },
            CbKind::Asap(f) => f(),
            CbKind::Interval(_, _) => {
                winuser::KillTimer(wnd, wparam);
            },
        }
        0
    } else {
        winuser::DefWindowProcA(wnd, msg, wparam, lparam)
    }
} 

impl<'a> Backend<'a> {
    pub (crate) fn new() -> Result<(Self, Box<SendFnOnce>), MainLoopError> {
        ensure_window_class();
        //println!("call CreateWindowExA");
        let wnd = unsafe { winuser::CreateWindowExA(
            0,
            WINDOW_CLASS_NAME.as_ptr() as *const winnt::CHAR,
            b"Test\0".as_ptr() as *const winnt::CHAR,
            winuser::WS_OVERLAPPEDWINDOW,
            winuser::CW_USEDEFAULT,
            winuser::CW_USEDEFAULT,
            winuser::CW_USEDEFAULT,
            winuser::CW_USEDEFAULT,
            ptr::null_mut(),
            ptr::null_mut(),
            libloaderapi::GetModuleHandleA(ptr::null_mut()),
            ptr::null_mut()
        ) };
        // println!("call CreateWindowExA finish");
        // println!("wnd: {:?}", wnd);
        assert!(!wnd.is_null());
        let ownd = Arc::new(OwnedHwnd(wnd));
        let be = Backend { wnd: ownd.clone(), _z: PhantomData };
        Ok((be, Box::new(ownd)))
    }
    pub fn run_one(&self, wait: bool) -> bool {
        unsafe {
            let mut msg = mem::zeroed();
            if winuser::PeekMessageA(&mut msg, self.wnd.0, 0, 0, winuser::PM_REMOVE) != 0 {
                winuser::TranslateMessage(&msg);
                winuser::DispatchMessageA(&msg);
                true
            } else if wait {
                winuser::WaitMessage();
                false
            } else { false }
        }
    }
    pub (crate) fn push(&self, cb: CbKind<'a>) -> Result<CbId, MainLoopError> {
        let d = cb.duration_millis()?;
        let x = Box::into_raw(Box::new(cb));
        match d {
            None => unsafe { 
                winuser::PostMessageA(self.wnd.0, WM_CALL_ASAP, x as usize, 0);
            },
            Some(d) => unsafe {
                winuser::SetTimer(self.wnd.0, x as usize, d, None);
            }
        };
        Ok(CbId())
    }
}
