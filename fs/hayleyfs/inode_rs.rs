#![allow(non_camel_case_types)]
#![allow(missing_docs)]
#![allow(non_upper_case_globals)]

use kernel::bindings::inode_operations;
use kernel::c_default_struct;
use kernel::c_types::c_void;

// pub(crate) makes it visible to the whole crate
// not sure why it is not already visible with in the crate...?
pub(crate) static hayleyfs_dir_inode_operations: inode_operations = inode_operations {
    ..c_default_struct!(inode_operations)
};

// inode that lives in
// TODO: should this actually be packed?
#[repr(packed)]
struct hayleyfs_inode {
    data0: pm_page,
    data1: pm_page,
    data2: pm_page,
    data3: pm_page,
    inum: u64,
    mode: u64, // should be smaller, but whatever
}

struct pm_page {
    page: *const c_void,
}
