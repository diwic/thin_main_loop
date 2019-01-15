use crate::{CbKind, CbId, MainLoopError};

use js_sys::Promise;
use wasm_bindgen::{JsValue, closure::Closure};
use std::cell::RefCell;

pub (crate) fn call_internal(cb: CbKind<'static>) -> Result<CbId, MainLoopError> {
    let d = cb.duration_millis()?;
    match cb {
        CbKind::Asap(f) => {
            let f2 = RefCell::new(Some(f));
            let id = Closure::wrap(Box::new(move |_| {
                let f = f2.borrow_mut().take().unwrap();
                f();
            }) as Box<dyn FnMut(JsValue)>);
            Promise::resolve(&JsValue::TRUE).then(&id);
            id.forget()
        }
        _ => unimplemented!(),
    }
    Ok(CbId())
}
