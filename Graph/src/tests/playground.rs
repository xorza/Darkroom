use std::thread;

use tokio::runtime::Runtime;

#[test]
fn test1() {
    let rt = Runtime::new().unwrap();

    let _handle = {
        let _guard = rt.enter();

        tokio::spawn(async {
            for i in 0..10000 {
                println!("Hello world {}", i);
                tokio::task::yield_now().await;
            }
        })
    };
    thread::sleep(std::time::Duration::from_millis(1));
    rt.shutdown_background();
    // rt.block_on(handle).unwrap();
}


