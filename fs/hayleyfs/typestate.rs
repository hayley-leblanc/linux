/// Zero-sized types for persistence state
#[derive(Debug)]
pub(crate) struct Dirty {}
#[derive(Debug)]
pub(crate) struct InFlight {}
#[derive(Debug)]
pub(crate) struct Clean {}

// TODO: maybe have op-specific complete states?

/// Zero-sized types for operation state
#[derive(Debug)]
pub(crate) struct Start {}
#[derive(Debug)]
pub(crate) struct Free {}
#[derive(Debug)]
pub(crate) struct Alloc {}
pub(crate) struct Init {}
pub(crate) struct IncLink {}
#[derive(Debug)]
pub(crate) struct Complete {}
#[derive(Debug)]
pub(crate) struct Writeable {}
#[derive(Debug)]
pub(crate) struct Written {}
pub(crate) struct IncSize {}
pub(crate) struct ClearIno {}
pub(crate) struct DecLink {}
pub(crate) struct Dealloc {}
pub(crate) struct ToUnmap {}
pub(crate) struct SetRenamePointer {}
pub(crate) struct InitRenamePointer {}
// pub(crate) struct ClearRenamePointer {}
pub(crate) struct Renaming {}
pub(crate) struct Renamed {}
pub(crate) struct UnmapPages {}

/// Traits to allow a transition from multiple legal typestates
pub(crate) trait Initialized {}
impl Initialized for Init {}
impl Initialized for Start {}
impl Initialized for Complete {}
impl Initialized for Written {}
impl Initialized for Writeable {} // FIXME: potential issue - new pages could be added to the index before they are written to
                                  // but the typestates are tricky especially during remount so making writeable pages indexable
                                  // is the easiest thing to do for now

pub(crate) trait AddLink {}
impl AddLink for Alloc {}
impl AddLink for IncLink {}

// undescriptive name because this is used in multiple unrelated places
// 1. setting a rename pointer
// 2. setting a data page backpointer
pub(crate) trait StartOrAlloc {}
impl StartOrAlloc for Start {}
impl StartOrAlloc for Alloc {}

pub(crate) trait DeletableDentry {}
impl DeletableDentry for Start {}
impl DeletableDentry for Renamed {}

pub(crate) trait CanDeletePages {}
impl CanDeletePages for ClearIno {}
impl CanDeletePages for Complete {}

pub(crate) trait InvalidInode {}
impl InvalidInode for DecLink {}
impl InvalidInode for Free {}
impl InvalidInode for Complete {} // TODO: this may not be safe. some Complete inodes are invalid but some aren't
