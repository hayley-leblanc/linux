#![deny(unused_must_use)]
#![deny(unused_variables)]
#![deny(clippy::let_underscore_must_use)]

use crate::def::*;
use crate::pm::*;
use crate::super_def::*;
use core::marker::PhantomData;
use core::ptr;
use kernel::prelude::*;
use kernel::PAGE_SIZE;

pub(crate) mod hayleyfs_data {
    use super::*;

    // generic page structure that can be used to represent any page
    // without a known structure
    struct DataPage {
        data: [u8; PAGE_SIZE],
    }

    pub(crate) struct DataPageWrapper<'a, State, Op> {
        state: PhantomData<State>,
        op: PhantomData<Op>,
        data_page: &'a mut DataPage,
    }

    impl<'a, State, Op> PmObjWrapper for DataPageWrapper<'a, State, Op> {}

    impl<'a, State, Op> PmObjWrapper for Vec<DataPageWrapper<'a, State, Op>> {}

    impl<'a, State, Op> DataPageWrapper<'a, State, Op> {
        fn new(data_page: &'a mut DataPage) -> Self {
            Self {
                state: PhantomData,
                op: PhantomData,
                data_page,
            }
        }
    }

    impl<'a> DataPageWrapper<'a, Clean, Read> {
        pub(crate) fn read_data_page(sbi: &SbInfo, page_no: PmPage) -> Result<Self> {
            check_page_no(sbi, page_no)?;
            let addr = (sbi.virt_addr as usize) + (PAGE_SIZE * page_no);
            Ok(DataPageWrapper::new(unsafe {
                &mut *(addr as *mut DataPage)
            }))
        }

        pub(crate) fn zero_page(self) -> DataPageWrapper<'a, Flushed, Zero> {
            // unsafely zero the memory region associated with this page
            // TODO: do this with nontemporal stores rather than cache line flushes
            unsafe { ptr::write_bytes(&mut self.data_page.data, 0, PAGE_SIZE) };
            clwb(&self.data_page.data, PAGE_SIZE, false);
            DataPageWrapper::new(self.data_page)
        }
    }

    impl<'a, Op> DataPageWrapper<'a, Flushed, Op> {
        pub(crate) unsafe fn fence_unsafe(self) -> DataPageWrapper<'a, Clean, Op> {
            DataPageWrapper::new(self.data_page)
        }
    }
}
