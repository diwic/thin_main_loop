use crate::{CbKind, CbId, MainLoopError, IODirection};
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

struct BeInternal<'a> {
    wnd: Arc<OwnedHwnd>,
    cb_map: RefCell<HashMap<CbId, CbKind<'a>>>,
    socket_map: RefCell<HashMap<usize, CbId>>,
}

impl<'a> BeInternal<'a> {
    fn call_data(&self, cbid: CbId, dir: Option<Result<IODirection, std::io::Error>>) -> bool {
        let kind = self.cb_map.borrow_mut().remove(&cbid);
        if let Some(mut kind) = kind {
            if kind.call_mut(dir) {
                self.cb_map.borrow_mut().insert(cbid, kind);
                return true;
            }
            self.remove(cbid, &kind);
            kind.post_call_mut();
        }
        false
    }

    fn remove(&self, cbid: CbId, kind: &CbKind<'a>) {
        if let Some((sock, _)) = kind.socket() {
            let sock = sock as usize;
            unsafe { winsock2::WSAAsyncSelect(sock, self.wnd.0, WM_SOCKET, 0); }
            self.socket_map.borrow_mut().remove(&sock);
        }
        if let Some(_) = kind.duration() {
            unsafe { winuser::KillTimer(self.wnd.0, cbid.0 as usize); }
        }
    }
}

// Boxed because we need the pointer not to move in callbacks
pub struct Backend<'a>(Box<BeInternal<'a>>);

impl SendFnOnce for Arc<OwnedHwnd> {
    fn send(&self, f: SendBoxFnOnce<'static, ()>) -> Result<(), MainLoopError> {
        let cb = CbKind::Asap(f.into());
        let x = Box::into_raw(Box::new(cb));
        unsafe {
            winuser::PostMessageA(self.0, WM_CALL_THREAD, x as usize, 0);
        }
        Ok(())
    }
}

const WM_CALL_ASAP: u32 = winuser::WM_USER + 10;
const WM_SOCKET: u32 = winuser::WM_USER + 11;
const WM_CALL_THREAD: u32 = winuser::WM_USER + 12;
static WINDOW_CLASS: Once = Once::new();
static WINDOW_CLASS_NAME: &[u8] = b"Rust function dispatch\0";

fn ensure_window_class() {
    WINDOW_CLASS.call_once(|| unsafe {
        let mut wc: winuser::WNDCLASSA = mem::zeroed();
        wc.lpszClassName = WINDOW_CLASS_NAME.as_ptr() as *const winnt::CHAR;
        wc.hInstance = libloaderapi::GetModuleHandleA(ptr::null_mut());
        wc.lpfnWndProc = Some(wnd_callback);
        winuser::RegisterClassA(&wc);
    });
}

unsafe extern "system" fn wnd_callback(wnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_SOCKET => {},
        winuser::WM_TIMER => {},
        WM_CALL_ASAP => {},
        WM_CALL_THREAD => {},
        _ => {
            return winuser::DefWindowProcA(wnd, msg, wparam, lparam);
        }
    };

    ffi_cb_wrapper(|| {
        let be = winuser::GetWindowLongPtrW(wnd, winuser::GWLP_USERDATA);
        assert!(be != 0);
        let be: &BeInternal = mem::transmute(be);
        match msg {
            WM_SOCKET => {
                // println!("WM_Socket: {} {}", wparam, lparam);
                let dir = match (lparam as i32) & (winsock2::FD_READ | winsock2::FD_WRITE) {
                    0 => Ok(IODirection::None),
                    winsock2::FD_READ => Ok(IODirection::Read),
                    winsock2::FD_WRITE => Ok(IODirection::Write),
                    _ => Ok(IODirection::Both),
                };
                let cbid = *be.socket_map.borrow().get(&wparam).unwrap();
                if !be.call_data(cbid, Some(dir)) {
                    winsock2::WSAAsyncSelect(wparam, wnd, WM_SOCKET, 0);
                    be.socket_map.borrow_mut().remove(&wparam);
                }
            },
            winuser::WM_TIMER | WM_CALL_ASAP => {
                let cbid = CbId(wparam as u64);
                be.call_data(cbid, None);
            },
            WM_CALL_THREAD => {
                let mut kind: Box<CbKind<'static>> = Box::from_raw(wparam as *mut _);
                assert!(!kind.call_mut(None));
                kind.post_call_mut();
            }
            _ => unreachable!(),
        };
        0
    }, 0)
} 

impl<'a> Drop for Backend<'a> {
    fn drop(&mut self) {
        unsafe { winuser::SetWindowLongPtrW(self.0.wnd.0, winuser::GWLP_USERDATA, 0) };
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
        let be = Box::new(BeInternal {
            wnd: ownd.clone(),
            cb_map: Default::default(),
            socket_map: Default::default()
        });
        unsafe {
            let be_ptr: &BeInternal = &be;
            let be_ptr = mem::transmute(be_ptr);
            winuser::SetWindowLongPtrW(wnd, winuser::GWLP_USERDATA, be_ptr);
        };
        Ok((Backend(be), Box::new(ownd)))
    }

    pub fn run_one(&self, wait: bool) -> bool {
        unsafe {
            let mut msg = mem::zeroed();
            if winuser::PeekMessageW(&mut msg, self.0.wnd.0, 0, 0, winuser::PM_REMOVE) != 0 {
                winuser::TranslateMessage(&msg);
                winuser::DispatchMessageW(&msg);
                true
            } else if wait {
                winuser::WaitMessage();
                false
            } else { false }
        }
    }

    pub (crate) fn cancel(&self, cbid: CbId) -> Option<CbKind<'a>> {
        let z = self.0.cb_map.borrow_mut().remove(&cbid);
        if let Some(cb) = z.as_ref() {
            self.0.remove(cbid, cb);
        };
        z
    }

    pub (crate) fn push(&self, cbid: CbId, cb: CbKind<'a>) -> Result<(), MainLoopError> {
        assert!(cbid.0 <= std::usize::MAX as u64);
        let cbu = cbid.0 as usize;
        let wnd = self.0.wnd.0;
        if let Some((socket, direction)) = cb.socket() {
            let events = match direction {
                IODirection::None => 0,
                IODirection::Read => winsock2::FD_READ,
                IODirection::Write => winsock2::FD_WRITE,
                IODirection::Both => winsock2::FD_READ | winsock2::FD_WRITE,
            } + winsock2::FD_CLOSE;
            let sock = socket as usize;
            unsafe { winsock2::WSAAsyncSelect(sock, wnd, WM_SOCKET, events) };
            self.0.socket_map.borrow_mut().insert(sock, cbid);
        } else if let Some(d) = cb.duration_millis()? {
            unsafe { winuser::SetTimer(wnd, cbu, d, None); }
        } else {
            unsafe { winuser::PostMessageW(wnd, WM_CALL_ASAP, cbu, 0); }
        };
        self.0.cb_map.borrow_mut().insert(cbid, cb);
        Ok(())
    }
}
