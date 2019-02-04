use std::marker::PhantomData;
use crate::{CbKind, CbId, MainLoopError, IODirection, IOAble};
use crate::mainloop::{SendFnOnce, ffi_cb_wrapper};
use winapi;
use std::{mem, ptr};
use std::sync::{Once, Arc};
use std::collections::HashMap;
use std::cell::RefCell;
use boxfnonce::SendBoxFnOnce;

use winapi::shared::windef::HWND;
use winapi::um::winuser;
use winapi::um::libloaderapi;
use winapi::um::winnt;
use winapi::um::winsock2;

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
    fn send(&self, f: SendBoxFnOnce<'static, ()>) -> Result<(), MainLoopError> {
        let cb = CbKind::Asap(f.into());
        let x = Box::into_raw(Box::new(cb));
        unsafe {
            winuser::PostMessageA(self.0, WM_CALL_ASAP, x as usize, 0);
        }
        Ok(())
    }
}

const WM_CALL_ASAP: u32 = winuser::WM_USER + 10;
const WM_SOCKET: u32 = winuser::WM_USER + 11;
static WINDOW_CLASS: Once = Once::new();
static WINDOW_CLASS_NAME: &[u8] = b"Rust function dispatch\0";

thread_local! {
    static sockets: RefCell<HashMap<winsock2::SOCKET, *mut IOAble>> = Default::default();

}

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
    ffi_cb_wrapper(|| {
        if msg == WM_SOCKET {
            println!("WM_Socket: {} {}", wparam, lparam);
            let dir = match (lparam as i32) & (winsock2::FD_READ | winsock2::FD_WRITE) {
                0 => Ok(IODirection::None),
                winsock2::FD_READ => Ok(IODirection::Read),
                winsock2::FD_WRITE => Ok(IODirection::Write),
                _ => Ok(IODirection::Both),
            };
            // FIXME: And what about socket errors? 
            sockets.with(|s| {
                if let Some(io) = s.borrow_mut().get_mut(&wparam) {
                    let x: &mut IOAble = &mut (**io);
                    x.on_rw(dir);
                } else { unreachable!(); }
            });
            0
        }
        else if msg == WM_CALL_ASAP || msg == winuser::WM_TIMER {
            let x = wparam as *mut CbKind;
            if (*x).call_mut(None) { return 0; }
            // Final call...
            let x = Box::from_raw(x);
            if x.duration().is_some() {
                winuser::KillTimer(wnd, wparam);
            }
            x.post_call_mut(); 
            0
        } else {
            winuser::DefWindowProcA(wnd, msg, wparam, lparam)
        }
    }, 0)
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
    pub (crate) fn cancel(&self, cbid: CbId) -> Option<CbKind<'a>> { unimplemented!() }
    pub (crate) fn push(&self, cbid: CbId, cb: CbKind<'a>) -> Result<(), MainLoopError> {
        if let CbKind::IO(io) = cb {
            let events = match io.direction() {
                IODirection::None => 0,
                IODirection::Read => winsock2::FD_READ,
                IODirection::Write => winsock2::FD_WRITE,
                IODirection::Both => winsock2::FD_READ | winsock2::FD_WRITE,
            } + winsock2::FD_CLOSE;
            let sock = io.socket();
            unsafe { winsock2::WSAAsyncSelect(sock as usize, self.wnd.0, WM_SOCKET, events) };
            sockets.with(|s| {
                let x: *mut (dyn IOAble + 'a) = Box::into_raw(io);
                // FIXME: We transmute from 'a to 'static here, how safe is that?
                let x: *mut (dyn IOAble + 'static) = unsafe { mem::transmute(x) };
                s.borrow_mut().insert(sock as usize, x);
            });
            return Ok(());
        }
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
        Ok(())
    }
}
