// TODO: remove debug derivations - they probably make these types non-zero-sized

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
#[derive(Debug)]
pub(crate) struct Init {}
#[derive(Debug)]
pub(crate) struct IncLink {}
#[derive(Debug)]
pub(crate) struct Complete {}
#[derive(Debug)]
pub(crate) struct Writeable {}
#[derive(Debug)]
pub(crate) struct Written {}
#[derive(Debug)]
pub(crate) struct IncSize {}
#[derive(Debug)]
pub(crate) struct ClearIno {}
#[derive(Debug)]
pub(crate) struct DecLink {}
#[derive(Debug)]
pub(crate) struct Dealloc {}
#[derive(Debug)]
pub(crate) struct ToUnmap {}

/// Traits to allow a transition from multiple legal typestates
pub(crate) trait Initialized {}
impl Initialized for Init {}
impl Initialized for Start {}
impl Initialized for Written {}
impl Initialized for Writeable {} // FIXME: potential issue - new pages could be added to the index before they are written to
                                  // but the typestates are tricky especially during remount so making writeable pages indexable
                                  // is the easiest thing to do for now

pub(crate) trait AddLink {}
impl AddLink for Alloc {}
impl AddLink for IncLink {}
