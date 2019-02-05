#![cfg(feature = "futures")]

extern crate thin_main_loop as tml;
extern crate futures;

#[test]
fn futures1() {
    let x = futures::future::lazy(|_| {
        println!("Called!");
        tml::terminate();
    });
    let mut ml = tml::MainLoop::new().unwrap();
    tml::spawn(x).unwrap();
    ml.run();
}

