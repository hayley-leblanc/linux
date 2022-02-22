use crate::data::*;
use crate::defs::*;
use crate::inode_rs::*;
use crate::pm::*;
use crate::super_def::*;
use core::mem::size_of;
use kernel::prelude::*;

// NOTE: fields of tokens defined in this file should ALWAYS be private.
// you should always be forced to create them via a constructor
// and modify them only in ways that it is safe to do so

// TODO: consider making a trait for each category of token (e.g., tokens
// that store cache lines, tokens that store inodes, etc.) I don't THINK
// we would get anything useful out of that right now, but it could be
// useful in the future

pub(crate) struct SuperInitToken<'a> {
    hsb: &'a mut HayleyfsSuperBlock,
}

impl<'a> SuperInitToken<'a> {
    pub(crate) fn new(hsb: &'a mut HayleyfsSuperBlock) -> Self {
        pr_info!("Flushing super init token!\n");
        clflush(hsb, size_of::<HayleyfsSuperBlock>(), false);
        Self { hsb }
    }
}

pub(crate) struct InodeAllocToken {
    ino: InodeNum,
    cache_line: *const CacheLine,
}

impl InodeAllocToken {
    /// TODO: can we avoid fencing here?
    pub(crate) fn new(i: InodeNum, line: *mut CacheLine) -> Self {
        pr_info!("flushing inode alloc token\n");
        clflush(line, CACHELINE_SIZE, true);
        Self {
            ino: i,
            cache_line: line,
        }
    }

    /// return the inode number associated with this token
    pub(crate) fn ino(&self) -> InodeNum {
        self.ino
    }
}

pub(crate) struct InodeInitToken<'a> {
    inode: &'a mut HayleyfsInode,
}

impl<'a> InodeInitToken<'a> {
    pub(crate) fn new(inode: &'a mut HayleyfsInode) -> Self {
        pr_info!("flushing inode init token!\n");
        clflush(inode, size_of::<HayleyfsInode>(), true);
        Self { inode }
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.inode.get_ino()
    }

    // TODO: should this be unsafe?
    pub(crate) fn get_inode(&mut self) -> &mut HayleyfsInode {
        self.inode
    }

    pub(crate) fn add_data_page(
        self,
        page: PmPage,
        init_token: &DirInitToken<'_>,
    ) -> DirPageAddToken<'a> {
        self.inode.set_data_page_no(Some(page));
        DirPageAddToken::new(self.inode, &init_token)
    }
}

pub(crate) struct ParentLinkToken<'a> {
    inode: &'a HayleyfsInode,
}

impl<'a> ParentLinkToken<'a> {
    pub(crate) fn new(inode: &'a mut HayleyfsInode) -> Self {
        pr_info!("flushing parent link token\n");
        clflush(inode, size_of::<HayleyfsInode>(), true);
        Self { inode }
    }
}

pub(crate) struct DataAllocToken {
    page_no: PmPage,
    cache_line: *const CacheLine,
}

impl DataAllocToken {
    pub(crate) fn new(p: PmPage, line: *mut CacheLine) -> Self {
        pr_info!("flushing alloc token for page {:?}\n", p);
        clflush(line, CACHELINE_SIZE, true);
        Self {
            page_no: p,
            cache_line: line,
        }
    }

    pub(crate) fn page_no(&self) -> PmPage {
        self.page_no
    }
}

pub(crate) struct DirInitToken<'a> {
    self_dentry: &'a HayleyfsDentry,
    parent_dentry: &'a HayleyfsDentry,
}

impl<'a> DirInitToken<'a> {
    pub(crate) fn new(s: &'a mut HayleyfsDentry, p: &'a mut HayleyfsDentry) -> Self {
        pr_info!("flushing dir init token!\n");
        // flush them separately in case there is some unexpected padding
        // this could cause redundant flushes
        clflush(s, size_of::<HayleyfsDentry>(), false);
        clflush(p, size_of::<HayleyfsDentry>(), true);
        Self {
            self_dentry: s,
            parent_dentry: p,
        }
    }
}

pub(crate) struct DentryAddToken<'a> {
    dentry: &'a HayleyfsDentry,
}

impl<'a> DentryAddToken<'a> {
    pub(crate) fn new(d: &'a mut HayleyfsDentry) -> Self {
        // TODO: fencing, and does this still have to be done in two flushes?
        // flush and fence the dentry
        pr_info!("flushing dentry add token\n");
        clflush(d, size_of::<HayleyfsDentry>(), true);
        // then make it valid
        unsafe { d.set_valid(true) };
        // then flush and fence again
        clflush(d, size_of::<HayleyfsDentry>(), true);
        Self { dentry: d }
    }
}

pub(crate) struct DirPageAddToken<'a> {
    inode: &'a HayleyfsInode,
}

impl<'a> DirPageAddToken<'a> {
    pub(crate) fn new(inode: &'a HayleyfsInode, _: &DirInitToken<'_>) -> Self {
        pr_info!("flushing dir page add token\n");
        clflush(inode, size_of::<HayleyfsInode>(), true);
        Self { inode }
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.inode.get_ino()
    }
}

// differs from dentry add token because this only provides an immutable
// reference to the dentry and does not flush on drop
pub(crate) struct DentryReadToken<'a> {
    dentry: &'a HayleyfsDentry,
}

impl<'a> DentryReadToken<'a> {
    pub(crate) unsafe fn new(d: &'a HayleyfsDentry) -> Self {
        Self { dentry: d }
    }

    pub(crate) fn get_ino(&self) -> InodeNum {
        self.dentry.get_ino()
    }
}
