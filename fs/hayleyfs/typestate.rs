/// Zero-sized types for persistence state
pub(crate) struct Dirty {}
pub(crate) struct InFlight {}
#[derive(Debug)]
pub(crate) struct Clean {}

// TODO: maybe have op-specific complete states?

/// Zero-sized types for operation state
#[derive(Debug)]
pub(crate) struct Start {}
pub(crate) struct Free {}
#[derive(Debug)]
pub(crate) struct Alloc {}
pub(crate) struct Init {}
pub(crate) struct IncLink {}
pub(crate) struct Complete {}
pub(crate) struct Writeable {}
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

pub(crate) trait AddPage {}
impl AddPage for Start {}
impl AddPage for Alloc {}

pub(crate) trait DeletableDentry {}
impl DeletableDentry for Start {}
impl DeletableDentry for Renamed {}
