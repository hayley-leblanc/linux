use crate::defs::*;
use crate::h_inode::*;
use crate::pm::*;
use crate::typestate::*;
use crate::volatile::*;
use core::{ffi, marker::PhantomData, mem};
use kernel::prelude::*;
use kernel::{bindings, dir, file, fs};

#[repr(C)]
#[derive(Debug)]
pub(crate) struct HayleyFsDentry {
    ino: InodeNum,
    name: [u8; MAX_FILENAME_LEN],
    rename_ptr: *mut HayleyFsDentry,
}

impl HayleyFsDentry {
    // Getters are not unsafe; only modifying HayleyFsDentry is unsafe
    pub(crate) fn get_ino(&self) -> InodeNum {
        self.ino
    }

    pub(crate) fn is_rename_ptr_null(&self) -> bool {
        self.rename_ptr.is_null()
    }

    pub(crate) fn has_name(&self) -> bool {
        for char in self.name {
            if char != 0 {
                return true;
            }
        }
        false
    }

    pub(crate) fn get_name(&self) -> [u8; MAX_FILENAME_LEN] {
        self.name
    }

    pub(crate) fn is_free(&self) -> bool {
        self.ino == 0 && self.is_rename_ptr_null() && !self.has_name()
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct DentryWrapper<'a, State, Op> {
    state: PhantomData<State>,
    op: PhantomData<Op>,
    dentry: &'a mut HayleyFsDentry,
}

impl<'a, State, Op> PmObjWrapper for DentryWrapper<'a, State, Op> {}

impl<'a> DentryWrapper<'a, Clean, Free> {
    /// Safety
    /// The provided dentry must be free (completely zeroed out).
    pub(crate) unsafe fn wrap_free_dentry(dentry: &'a mut HayleyFsDentry) -> Self {
        Self {
            state: PhantomData,
            op: PhantomData,
            dentry: dentry,
        }
    }

    /// CStr are guaranteed to have a `NUL` byte at the end, so we don't have to check
    /// for that.
    pub(crate) fn set_name(self, name: &CStr) -> Result<DentryWrapper<'a, Dirty, Alloc>> {
        if name.len() > MAX_FILENAME_LEN {
            return Err(ENAMETOOLONG);
        }
        // copy only the number of bytes in the name
        let num_bytes = if name.len() < MAX_FILENAME_LEN {
            name.len()
        } else {
            MAX_FILENAME_LEN
        };
        let name = name.as_bytes_with_nul();
        self.dentry.name[..num_bytes].clone_from_slice(&name[..num_bytes]);

        Ok(DentryWrapper {
            state: PhantomData,
            op: PhantomData,
            dentry: self.dentry,
        })
    }
}

impl<'a> DentryWrapper<'a, Clean, Alloc> {
    pub(crate) fn set_file_ino<State: AddLink>(
        self,
        inode: InodeWrapper<'a, Clean, State, RegInode>,
    ) -> (
        DentryWrapper<'a, Dirty, Complete>,
        InodeWrapper<'a, Clean, Complete, RegInode>,
    ) {
        self.dentry.ino = inode.get_ino();
        (
            DentryWrapper {
                state: PhantomData,
                op: PhantomData,
                dentry: self.dentry,
            },
            InodeWrapper::new(inode),
        )
    }

    pub(crate) fn set_dir_ino(
        self,
        new_inode: InodeWrapper<'a, Clean, Alloc, DirInode>,
        parent_inode: InodeWrapper<'a, Clean, IncLink, DirInode>,
    ) -> (
        DentryWrapper<'a, Dirty, Complete>,
        InodeWrapper<'a, Clean, Complete, DirInode>,
        InodeWrapper<'a, Clean, Complete, DirInode>,
    ) {
        self.dentry.ino = new_inode.get_ino();
        (
            DentryWrapper {
                state: PhantomData,
                op: PhantomData,
                dentry: self.dentry,
            },
            InodeWrapper::new(new_inode),
            InodeWrapper::new(parent_inode),
        )
    }
}

impl<'a, Op> DentryWrapper<'a, Clean, Op> {
    #[allow(dead_code)]
    pub(crate) fn get_ino(&self) -> InodeNum {
        self.dentry.get_ino()
    }
}

impl<'a> DentryWrapper<'a, Clean, Start> {
    pub(crate) fn get_init_dentry(info: DentryInfo) -> Result<Self> {
        // use the virtual address in the DentryInfo to look up the
        // persistent dentry
        let dentry: &mut HayleyFsDentry =
            unsafe { &mut *(info.get_virt_addr() as *mut HayleyFsDentry) };
        // return an error if the dentry is not initialized
        if dentry.ino == 0 {
            pr_info!("ERROR: dentry is invalid\n");
            return Err(EPERM);
        };
        Ok(Self {
            state: PhantomData,
            op: PhantomData,
            dentry,
        })
    }

    pub(crate) fn clear_ino(self) -> DentryWrapper<'a, Dirty, ClearIno> {
        self.dentry.ino = 0;
        DentryWrapper {
            state: PhantomData,
            op: PhantomData,
            dentry: self.dentry,
        }
    }
}

impl<'a> DentryWrapper<'a, Clean, ClearIno> {
    pub(crate) fn dealloc_dentry(self) -> DentryWrapper<'a, Dirty, Free> {
        self.dentry.name.iter_mut().for_each(|c| *c = 0);
        DentryWrapper {
            state: PhantomData,
            op: PhantomData,
            dentry: self.dentry,
        }
    }
}

impl<'a> DentryWrapper<'a, Clean, Complete> {
    // TODO: maybe should take completed inode as well? or ino dentry insert should
    // take the wrappers directly
    pub(crate) fn index(&self, parent_inode_info: &HayleyFsDirInodeInfo) -> Result<()> {
        let dentry_info = DentryInfo::new(
            self.dentry.ino,
            self.dentry as *const _ as *const ffi::c_void,
            self.dentry.name,
        );
        parent_inode_info.insert_dentry(dentry_info)
    }
}

impl<'a, Op> DentryWrapper<'a, Dirty, Op> {
    pub(crate) fn flush(self) -> DentryWrapper<'a, InFlight, Op> {
        flush_buffer(self.dentry, mem::size_of::<HayleyFsDentry>(), false);
        DentryWrapper {
            state: PhantomData,
            op: PhantomData,
            dentry: self.dentry,
        }
    }
}

impl<'a, Op> DentryWrapper<'a, InFlight, Op> {
    pub(crate) fn fence(self) -> DentryWrapper<'a, Clean, Op> {
        sfence();
        DentryWrapper {
            state: PhantomData,
            op: PhantomData,
            dentry: self.dentry,
        }
    }
}

pub(crate) struct DirOps;
#[vtable]
impl dir::Operations for DirOps {
    fn iterate(f: &file::File, ctx: *mut bindings::dir_context) -> Result<u32> {
        let inode: &mut fs::INode = unsafe { &mut *f.inode().cast() };
        let sb = inode.i_sb();
        let fs_info_raw = unsafe { (*sb).s_fs_info };
        // TODO: it's probably not safe to just grab s_fs_info and
        // get a mutable reference to one of the dram indexes
        let sbi = unsafe { &mut *(fs_info_raw as *mut SbInfo) };
        let result = hayleyfs_readdir(sbi, inode, ctx);
        match result {
            Ok(r) => Ok(r),
            Err(e) => Err(e),
        }
    }

    fn ioctl(data: (), file: &file::File, cmd: &mut file::IoctlCommand) -> Result<i32> {
        cmd.dispatch::<Self>(data, file)
    }
}

pub(crate) fn hayleyfs_readdir(
    sbi: &SbInfo,
    dir: &mut fs::INode,
    ctx: *mut bindings::dir_context,
) -> Result<u32> {
    // get all dentries currently in this inode
    // TODO: need to start at the specified position
    let (_parent_inode, parent_inode_info) =
        sbi.get_init_dir_inode_by_vfs_inode(dir.get_inner())?;
    let dentries = parent_inode_info.get_all_dentries()?;
    let num_dentries: i64 = dentries.len().try_into()?;
    unsafe {
        if (*ctx).pos >= num_dentries {
            return Ok(0);
        }
    }
    for dentry in dentries {
        let name =
            unsafe { CStr::from_char_ptr(dentry.get_name().as_ptr() as *const core::ffi::c_char) };
        let file_type = bindings::DT_UNKNOWN; // TODO: get the actual type
        let result = unsafe {
            bindings::dir_emit(
                ctx,
                name.as_char_ptr(),
                name.len().try_into()?,
                parent_inode_info.get_ino(),
                file_type,
            )
        };
        if !result {
            return Ok(0);
        }
    }
    unsafe { (*ctx).pos += num_dentries };
    Ok(0)
}
