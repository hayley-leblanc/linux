//! Test module to mess around with Rust in the kernel

use kernel::prelude::*;

module! {
    type: HayleyFS,
    name: b"hayleyfs",
    author: b"Hayley LeBlanc",
    description: b"Rust test fs module",
    license: b"GPL v2",
}

struct HayleyFS {
    message: String,
}

impl KernelModule for HayleyFS {
    fn init(_name: &'static CStr, _module: &'static ThisModule) -> Result<Self> {
        pr_info!("Hello! This is Hayley's Rust module!\n");
        pr_info!("Am I built-in? {}\n", !cfg!(MODULE));

        Ok(HayleyFS {
            message: "a string on the heap I guess?".try_to_owned()?,
        })
    }
}

impl Drop for HayleyFS {
    fn drop(&mut self) {
        pr_info!("My message is {}\n", self.message);
        pr_info!("Module is unloading. Goodbye!\n");
    }
}
