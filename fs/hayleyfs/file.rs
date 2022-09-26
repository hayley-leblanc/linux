use crate::{cdefs::*, super_def::*};
use core::ffi;
use kernel::prelude::*;
use kernel::rbtree::RBTree;
use kernel::sync::smutex::Mutex;

type PageNum = u64;

/// Trait for data page allocator to implement. Note that this only allocates and
/// deallocates page numbers - it does not persistently allocate/deallocate pages.
/// TODO: if we want to mess around with differently-sized allocations, this
/// will have to be changed a bit
pub(crate) trait PageAllocator {
    fn alloc_page(&mut self) -> Result<PageNum>;
    fn dealloc_page(&mut self, page_num: PageNum) -> Result<()>;
}

/// Simple bitmap for page allocation. This lives in DRAM
#[repr(C)]
pub(crate) struct PageBitmap(Mutex<[u8; PAGE_BITMAP_SIZE]>);

impl PageBitmap {
    pub(crate) fn new() -> Result<Self> {
        let bitmap: [u8; PAGE_BITMAP_SIZE] = [0; PAGE_BITMAP_SIZE];
        // set bits for reserved pages
        // SAFETY: we just initialized the bitmap as an array of zeroes
        // the reserved pages should not exceed the size of the device
        unsafe {
            let bitmap_ptr = bitmap.as_ptr() as *const ffi::c_void;
            set_bit_helper(SUPER_BLOCK_PAGE.try_into()?, bitmap_ptr);
            for i in INODE_TABLE_START..INODE_TABLE_START + INODE_TABLE_SIZE {
                set_bit_helper(i.try_into()?, bitmap_ptr);
            }
        }
        Ok(Self(Mutex::new(bitmap)))
    }
}

impl PageAllocator for PageBitmap {
    fn alloc_page(&mut self) -> Result<PageNum> {
        let bitmap = self.0.lock();
        // find a zero bit in the bitmap
        let page_num = unsafe {
            find_next_zero_bit_le_helper(
                bitmap.as_ptr() as *const ffi::c_ulong,
                PAGE_BITMAP_SIZE.try_into()?,
                0,
            )
        };
        if page_num > PAGE_BITMAP_SIZE.try_into()? {
            Err(ENOSPC)
        } else {
            let set = unsafe {
                test_and_set_bit_le_helper(
                    page_num.try_into()?,
                    bitmap.as_ptr() as *const ffi::c_void,
                )
            };
            if set != 0 {
                pr_err!("ERROR: page number {} is already set\n", set);
                Err(EINVAL)
            } else {
                Ok(page_num)
            }
        }
    }

    fn dealloc_page(&mut self, page_num: PageNum) -> Result<()> {
        if page_num >= PAGE_BITMAP_SIZE.try_into()? {
            return Err(EINVAL);
        }
        let bitmap = self.0.lock();
        let set = unsafe {
            test_and_clear_bit_le_helper(
                page_num.try_into()?,
                bitmap.as_ptr() as *const ffi::c_void,
            )
        };
        if set == 0 {
            pr_err!("ERROR: page number {} is already free\n", page_num);
            Err(EINVAL)
        } else {
            Ok(())
        }
    }
}

/// Trait for page index structure to implement
pub(crate) trait DataIndex {
    fn insert_file(&mut self, file: InodeNum) -> Result<()>;
    fn insert_page(&mut self, file: InodeNum, page: PageNum, offset: u64) -> Result<()>;
    fn remove_file(&mut self, file: InodeNum) -> Result<()>;
    fn remove_page(&mut self, file: InodeNum, offset: u64) -> Result<()>;
    fn lookup(&mut self, file: InodeNum, offset: u64) -> Option<PageNum>;
}

/// Simple file data index structure. Uses two RB trees to map inodes -> a set of offsets
/// and offsets -> page numbers
pub(crate) struct RBDataTree(Mutex<RBTree<InodeNum, RBTree<u64, PageNum>>>);

impl RBDataTree {
    pub(crate) fn new() -> Self {
        Self(Mutex::new(RBTree::new()))
    }
}

impl DataIndex for RBDataTree {
    fn insert_file(&mut self, file: InodeNum) -> Result<()> {
        let mut tree = self.0.lock();
        let node = tree.get_mut(&file);
        if node.is_none() {
            // insert the file into the top level tree
            let offset_tree = RBTree::new();
            tree.try_insert(file, offset_tree)?;
            Ok(())
        } else {
            Err(EEXIST)
        }
    }

    fn insert_page(&mut self, file: InodeNum, page: PageNum, offset: u64) -> Result<()> {
        let mut tree = self.0.lock();
        let node = tree.get_mut(&file);
        if node.is_none() {
            return Err(ENOENT);
        } else if let Some(offset_tree) = node {
            // TODO: do we need to check if there was already a page with that offset?
            offset_tree.try_insert(offset, page)?;
        }
        Ok(())
    }

    fn remove_file(&mut self, file: InodeNum) -> Result<()> {
        // TODO: we should probably check that pages have been removed/deallocated
        // before deleting them from the tree?
        let mut tree = self.0.lock();
        let result = tree.remove(&file);
        if result.is_none() {
            Err(ENOENT)
        } else {
            Ok(())
        }
    }

    fn remove_page(&mut self, file: InodeNum, offset: u64) -> Result<()> {
        let mut tree = self.0.lock();
        let node = tree.get_mut(&file);
        if let Some(offset_tree) = node {
            let result = offset_tree.remove(&offset);
            if result.is_none() {
                Err(EINVAL)
            } else {
                Ok(())
            }
        } else {
            Err(ENOENT)
        }
    }

    fn lookup(&mut self, file: InodeNum, offset: u64) -> Option<PageNum> {
        let tree = self.0.lock();
        let node = tree.get(&file);
        if let Some(offset_tree) = node {
            let page_num = offset_tree.get(&offset);
            page_num.map(|p| *p) // concisely dereference the page # within the option
        } else {
            None
        }
    }
}
