/// Zero-sized types for persistence state
pub(crate) struct Dirty {}
pub(crate) struct InFlight {}
pub(crate) struct Clean {}

/// Zero-sized types for operation state
pub(crate) struct Start {}
pub(crate) struct Free {}
pub(crate) struct Alloc {}
pub(crate) struct Init {}
pub(crate) struct Complete {}

/// Traits to allow a transition from multiple legal typestates
pub(crate) trait Initialized {}
impl Initialized for Init {}
impl Initialized for Start {}
