use crate::balloc::*;
use crate::defs::*;
use crate::h_dir::*;
use crate::typestate::*;
use core::ffi;
use kernel::prelude::*;
use kernel::{
    rbtree::RBTree,
    sync::{smutex::Mutex, Arc},
};

// TODO: how should name be represented here? array is probably not the best?
#[allow(dead_code)]
#[derive(Debug, Copy, Clone)]
pub(crate) struct DentryInfo {
    ino: InodeNum,
    virt_addr: *const ffi::c_void,
    name: [u8; MAX_FILENAME_LEN],
}

#[allow(dead_code)]
impl DentryInfo {
    pub(crate) fn new(
        ino: InodeNum,
        virt_addr: *const ffi::c_void,
        name: [u8; MAX_FILENAME_LEN],
    ) -> Self {
        Self {
            ino,
            virt_addr,
            name,
        }
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.ino
    }

    pub(crate) fn get_virt_addr(&self) -> *const ffi::c_void {
        self.virt_addr
    }

    pub(crate) fn get_name(&self) -> &[u8; MAX_FILENAME_LEN] {
        &self.name
    }
}

// /// maps inodes to info about dentries for inode's children
// pub(crate) trait InoDentryMap {
//     fn new() -> Result<Self>
//     where
//         Self: Sized;
//     fn insert(&self, ino: InodeNum, dentry: DentryInfo) -> Result<()>;
//     fn lookup_dentry(&self, ino: &InodeNum, name: &CStr) -> Option<DentryInfo>;
//     fn delete(&self, ino: InodeNum, dentry: DentryInfo) -> Result<()>;
// }

#[allow(dead_code)]
pub(crate) struct InoDentryTree {
    map: Arc<Mutex<RBTree<InodeNum, RBTree<[u8; MAX_FILENAME_LEN], DentryInfo>>>>,
}

impl InoDentryTree {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            map: Arc::try_new(Mutex::new(RBTree::new()))?,
        })
    }

    pub(crate) fn insert(&self, ino: InodeNum, dentry: DentryInfo) -> Result<()> {
        let map = Arc::clone(&self.map);
        let mut map = map.lock();
        if let Some(ref mut node) = map.get_mut(&ino) {
            node.try_insert(dentry.name, dentry)?;
        } else {
            let mut tree = RBTree::new();
            tree.try_insert(dentry.name, dentry)?;
            map.try_insert(ino, tree)?;
        }
        Ok(())
    }

    pub(crate) fn remove(
        &self,
        ino: InodeNum,
    ) -> Option<RBTree<[u8; MAX_FILENAME_LEN], DentryInfo>> {
        let map = Arc::clone(&self.map);
        let mut map = map.lock();
        map.remove(&ino)
    }
}

#[derive(Debug, Copy, Clone, PartialOrd, Eq, PartialEq, Ord)]
pub(crate) struct DirPageInfo {
    // owner: InodeNum,
    page_no: PageNum,
    // full: bool,
    // virt_addr: *mut ffi::c_void,
}

impl DirPageInfo {
    pub(crate) fn get_page_no(&self) -> PageNum {
        self.page_no
    }

    pub(crate) fn new(page_no: PageNum) -> Self {
        Self { page_no }
    }
}

#[derive(Debug, Copy, Clone)]
#[allow(dead_code)]
pub(crate) struct DataPageInfo {
    owner: InodeNum,
    page_no: PageNum,
    offset: u64,
}

impl DataPageInfo {
    pub(crate) fn new(owner: InodeNum, page_no: PageNum, offset: u64) -> Self {
        Self {
            owner,
            page_no,
            offset,
        }
    }

    pub(crate) fn get_page_no(&self) -> PageNum {
        self.page_no
    }
}

// TODO: could just be offset and page number - storing whole DataPageInfo is redundant...
#[repr(C)]
pub(crate) struct HayleyFsRegInodeInfo {
    ino: InodeNum,
    pages: Arc<Mutex<RBTree<u64, DataPageInfo>>>,
}

impl HayleyFsRegInodeInfo {
    pub(crate) fn new(ino: InodeNum) -> Result<Self> {
        Ok(Self {
            ino,
            pages: Arc::try_new(Mutex::new(RBTree::new()))?,
        })
    }

    pub(crate) fn new_from_tree(ino: InodeNum, tree: RBTree<u64, DataPageInfo>) -> Result<Self> {
        Ok(Self {
            ino,
            pages: Arc::try_new(Mutex::new(tree))?,
        })
    }
}

/// maps file inodes to info about their pages
pub(crate) trait InoDataPageMap {
    fn new(ino: InodeNum) -> Result<Self>
    where
        Self: Sized;
    fn insert<'a, State: Initialized>(
        &self,
        page: &DataPageWrapper<'a, Clean, State>,
    ) -> Result<()>;
    fn find(&self, offset: u64) -> Option<DataPageInfo>;
    fn get_all_pages(&self) -> Result<RBTree<u64, DataPageInfo>>;
    // fn delete(&self) -> Result<DataPageInfo>;
}

impl InoDataPageMap for HayleyFsRegInodeInfo {
    fn new(ino: InodeNum) -> Result<Self> {
        HayleyFsRegInodeInfo::new(ino)
    }

    fn insert<'a, State: Initialized>(
        &self,
        page: &DataPageWrapper<'a, Clean, State>,
    ) -> Result<()> {
        let pages = Arc::clone(&self.pages);
        let mut pages = pages.lock();
        let offset = page.get_offset();
        pages.try_insert(
            offset,
            DataPageInfo {
                owner: self.ino,
                page_no: page.get_page_no(),
                offset,
            },
        )?;
        Ok(())
    }

    fn find(&self, offset: u64) -> Option<DataPageInfo> {
        let pages = Arc::clone(&self.pages);
        let pages = pages.lock();
        // let index: usize = (offset / HAYLEYFS_PAGESIZE).try_into().unwrap();
        let result = pages.get(&offset);
        match result {
            Some(page) => Some(page.clone()),
            None => None,
        }
    }

    fn get_all_pages(&self) -> Result<RBTree<u64, DataPageInfo>> {
        let pages = Arc::clone(&self.pages);
        let pages = pages.lock();
        let mut return_tree = RBTree::new();
        // TODO: can you do this without copying all of the pages?
        for offset in pages.keys() {
            return_tree.try_insert(*offset, pages.get(offset).unwrap().clone())?;
        }
        Ok(return_tree)
    }

    // /// Deletes the last page in the file from the index and returns it
    // fn delete(&self) -> Result<DataPageInfo> {
    //     let pages = Arc::clone(&self.pages);
    //     let mut pages = pages.lock();
    //     pages.pop().ok_or(EINVAL)
    // }
}

/// maps dir inodes to info about their pages
pub(crate) trait InoDirPageMap {
    fn new(ino: InodeNum) -> Result<Self>
    where
        Self: Sized;
    fn insert<'a, State: Initialized>(&self, page: &DirPageWrapper<'a, Clean, State>)
        -> Result<()>;
    fn insert_page_infos(&self, new_pages: RBTree<DirPageInfo, ()>) -> Result<()>;
    fn find_page_with_free_dentry(&self, sbi: &SbInfo) -> Result<Option<DirPageInfo>>;
    fn get_all_pages(&self) -> Result<RBTree<DirPageInfo, ()>>;
    fn delete(&self, page: DirPageInfo) -> Result<()>;
}

#[repr(C)]
pub(crate) struct HayleyFsDirInodeInfo {
    ino: InodeNum,
    pages: Arc<Mutex<RBTree<DirPageInfo, ()>>>,
    dentries: Arc<Mutex<RBTree<[u8; MAX_FILENAME_LEN], DentryInfo>>>, // dentries: Arc<Mutex<Vec<DentryInfo>>>,
}

impl HayleyFsDirInodeInfo {
    pub(crate) fn new(ino: InodeNum) -> Result<Self> {
        Ok(Self {
            ino,
            pages: Arc::try_new(Mutex::new(RBTree::new()))?,
            dentries: Arc::try_new(Mutex::new(RBTree::new()))?,
        })
    }

    pub(crate) fn new_from_tree(
        ino: InodeNum,
        page_tree: RBTree<DirPageInfo, ()>,
        dentry_tree: RBTree<[u8; MAX_FILENAME_LEN], DentryInfo>,
    ) -> Result<Self> {
        Ok(Self {
            ino,
            pages: Arc::try_new(Mutex::new(page_tree))?,
            dentries: Arc::try_new(Mutex::new(dentry_tree))?,
        })
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.ino
    }
}

impl InoDirPageMap for HayleyFsDirInodeInfo {
    fn new(ino: InodeNum) -> Result<Self> {
        Self::new(ino)
    }

    fn insert<'a, State: Initialized>(
        &self,
        page: &DirPageWrapper<'a, Clean, State>,
    ) -> Result<()> {
        let pages = Arc::clone(&self.pages);
        let mut pages = pages.lock();
        // TODO: sort?
        let page_info = DirPageInfo {
            page_no: page.get_page_no(),
        };
        pages.try_insert(page_info, ())?;
        Ok(())
    }

    fn insert_page_infos(&self, new_pages: RBTree<DirPageInfo, ()>) -> Result<()> {
        let pages = Arc::clone(&self.pages);
        let mut pages = pages.lock();
        // TODO: sort?
        for (key, value) in new_pages.iter() {
            pages.try_insert(*key, *value)?;
        }
        Ok(())
    }

    // TODO: this only works because we don't ever deallocate dir pages right now
    // there could be a race between a process that is deleting a dir page and a
    // process trying to add a dentry to it. this method should just add the dentry
    fn find_page_with_free_dentry<'a>(&self, sbi: &SbInfo) -> Result<Option<DirPageInfo>> {
        let pages = Arc::clone(&self.pages);
        let pages = pages.lock();
        for page in pages.keys() {
            let p = DirPageWrapper::from_page_no(sbi, page.get_page_no())?;
            if p.has_free_space(sbi)? {
                return Ok(Some(page.clone()));
            }
        }

        Ok(None)
    }

    fn get_all_pages(&self) -> Result<RBTree<DirPageInfo, ()>> {
        let pages = Arc::clone(&self.pages);
        let pages = pages.lock();
        let mut return_tree = RBTree::new();
        // TODO: can you do this without copying all of the pages?
        for page in pages.keys() {
            return_tree.try_insert(page.clone(), ())?;
        }
        Ok(return_tree)
    }

    // TODO: implement
    fn delete(&self, _page: DirPageInfo) -> Result<()> {
        unimplemented!();
    }
}

pub(crate) trait InoDentryMap {
    fn insert_dentry(&self, dentry: DentryInfo) -> Result<()>;
    fn insert_dentries(
        &self,
        new_dentries: RBTree<[u8; MAX_FILENAME_LEN], DentryInfo>,
    ) -> Result<()>;
    fn lookup_dentry(&self, name: &CStr) -> Option<DentryInfo>;
    fn get_all_dentries(&self) -> Result<Vec<DentryInfo>>;
    fn delete_dentry(&self, dentry: DentryInfo) -> Result<()>;
    fn atomic_add_and_delete_dentry<'a>(
        &self,
        new_dentry: &DentryWrapper<'a, Clean, Complete>,
        old_dentry: &[u8; MAX_FILENAME_LEN],
    ) -> Result<()>;
    fn is_empty(&self) -> bool;
}

impl InoDentryMap for HayleyFsDirInodeInfo {
    fn insert_dentry(&self, dentry: DentryInfo) -> Result<()> {
        let dentries = Arc::clone(&self.dentries);
        let mut dentries = dentries.lock();
        dentries.try_insert(dentry.name, dentry)?;
        Ok(())
    }

    fn insert_dentries(
        &self,
        new_dentries: RBTree<[u8; MAX_FILENAME_LEN], DentryInfo>,
    ) -> Result<()> {
        let dentries = Arc::clone(&self.dentries);
        let mut dentries = dentries.lock();
        for name in new_dentries.keys() {
            // janky hack to fill in the tree. ideally we could do this
            // without iterating.
            // it is safe to unwrap because we know name is a key and we have
            // ownership of new_dentries
            dentries.try_insert(*name, *new_dentries.get(name).unwrap())?;
        }
        Ok(())
    }

    fn lookup_dentry(&self, name: &CStr) -> Option<DentryInfo> {
        let dentries = Arc::clone(&self.dentries);
        let dentries = dentries.lock();
        // TODO: can you do this without creating the array?
        let mut full_filename = [0; MAX_FILENAME_LEN];
        full_filename[..name.len()].copy_from_slice(name.as_bytes());
        let dentry = dentries.get(&full_filename);
        dentry.copied()
    }

    fn get_all_dentries(&self) -> Result<Vec<DentryInfo>> {
        let dentries = Arc::clone(&self.dentries);
        let dentries = dentries.lock();
        let mut return_vec = Vec::new();

        // TODO: use an iterator method
        for d in dentries.values() {
            return_vec.try_push(d.clone())?;
        }
        Ok(return_vec)
    }

    fn delete_dentry(&self, dentry: DentryInfo) -> Result<()> {
        let dentries = Arc::clone(&self.dentries);
        let mut dentries = dentries.lock();
        dentries.remove(&dentry.name);
        Ok(())
    }

    fn atomic_add_and_delete_dentry<'a>(
        &self,
        new_dentry: &DentryWrapper<'a, Clean, Complete>,
        // old_dentry: &DentryWrapper<'a, Clean, Free>,
        old_dentry_name: &[u8; MAX_FILENAME_LEN], // can't use actual dentry because it no longer has a name
    ) -> Result<()> {
        let new_dentry_info = new_dentry.get_dentry_info();
        // let old_dentry_info = old_dentry.get_dentry_info();
        let dentries = Arc::clone(&self.dentries);
        let mut dentries = dentries.lock();
        pr_info!("inserting {:?}\n", new_dentry_info);
        // pr_info!("removing {:?}\n", old_dentry_info);
        dentries.try_insert(new_dentry_info.name, new_dentry_info)?;
        dentries.remove(old_dentry_name);
        Ok(())
    }

    fn is_empty(&self) -> bool {
        let dentries = Arc::clone(&self.dentries);
        let dentries = dentries.lock();
        let mut keys = dentries.keys().peekable();
        keys.peek().is_none()
    }
}

pub(crate) trait PageInfo {}
impl PageInfo for DataPageInfo {}
impl PageInfo for DirPageInfo {}

pub(crate) struct InoDataPageTree {
    tree: Arc<Mutex<RBTree<InodeNum, RBTree<u64, DataPageInfo>>>>,
}

impl InoDataPageTree {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            tree: Arc::try_new(Mutex::new(RBTree::new()))?,
        })
    }

    pub(crate) fn insert_inode(
        &self,
        ino: InodeNum,
        pages: RBTree<u64, DataPageInfo>,
    ) -> Result<()> {
        let tree = Arc::clone(&self.tree);
        let mut tree = tree.lock();
        tree.try_insert(ino, pages)?;
        Ok(())
    }

    pub(crate) fn remove(&self, ino: InodeNum) -> Option<RBTree<u64, DataPageInfo>> {
        let tree = Arc::clone(&self.tree);
        let mut tree = tree.lock();
        tree.remove(&ino)
    }
}

pub(crate) struct InoDirPageTree {
    tree: Arc<Mutex<RBTree<InodeNum, RBTree<DirPageInfo, ()>>>>,
}

impl InoDirPageTree {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            tree: Arc::try_new(Mutex::new(RBTree::new()))?,
        })
    }

    pub(crate) fn insert_inode(&self, ino: InodeNum, pages: RBTree<DirPageInfo, ()>) -> Result<()> {
        let tree = Arc::clone(&self.tree);
        let mut tree = tree.lock();
        tree.try_insert(ino, pages)?;
        Ok(())
    }

    pub(crate) fn insert_one(&self, ino: InodeNum, page: DirPageInfo) -> Result<()> {
        let tree = Arc::clone(&self.tree);
        let mut tree = tree.lock();
        let entry = tree.get_mut(&ino);
        if let Some(entry) = entry {
            entry.try_insert(page, ())?;
        } else {
            let mut new_tree = RBTree::new();
            new_tree.try_insert(page, ())?;
            tree.try_insert(ino, new_tree)?;
        }
        Ok(())
    }

    pub(crate) fn remove(&self, ino: InodeNum) -> Option<RBTree<DirPageInfo, ()>> {
        let tree = Arc::clone(&self.tree);
        let mut tree = tree.lock();
        tree.remove(&ino)
    }
}

pub(crate) fn move_dir_inode_tree_to_map(
    sbi: &SbInfo,
    parent_inode_info: &HayleyFsDirInodeInfo,
) -> Result<()> {
    let ino = parent_inode_info.get_ino();
    let dentries = sbi.ino_dentry_tree.remove(ino);
    let dir_pages = sbi.ino_dir_page_tree.remove(ino);

    if let Some(dentries) = dentries {
        parent_inode_info.insert_dentries(dentries)?;
    }
    if let Some(dir_pages) = dir_pages {
        parent_inode_info.insert_page_infos(dir_pages)?;
    }
    Ok(())
}
